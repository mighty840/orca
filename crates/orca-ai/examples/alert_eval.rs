//! Send the alert scenarios to a real model and score the diagnoses.
//!
//! ```sh
//! export ORCA_AI_ENDPOINT=https://llm.example.com   # OpenAI-compatible, no /v1
//! export ORCA_AI_MODEL=gpt-oss-120b
//! export ORCA_AI_API_KEY=...                        # optional
//! cargo run -p orca-ai --example alert_eval                  # all scenarios
//! cargo run -p orca-ai --example alert_eval -- oom-killed    # one scenario
//! cargo run -p orca-ai --example alert_eval -- --old         # the pre-evidence prompt
//! cargo run -p orca-ai --example alert_eval -- --show-prompt # print what is sent
//! ```
//!
//! `--old` rebuilds the prompt the monitor sent before the evidence work: the
//! whole-cluster prompt, counts only, no logs or failure, and a free-form
//! question. Run both and compare.

#[path = "../tests/fixtures/alert_scenarios.rs"]
mod alert_scenarios;

use alert_scenarios::Scenario;
use orca_ai::backend::{ChatMessage, LlmBackend, OpenAiCompatibleBackend, Role};
use orca_ai::conversation::ConversationEngine;

const SECTIONS: &[&str] = &[
    "What happened",
    "Evidence",
    "Likely cause",
    "Fix",
    "Verify",
    "Resolves on its own",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let old = args.iter().any(|a| a == "--old");
    let show_prompt = args.iter().any(|a| a == "--show-prompt");
    let only: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();

    let endpoint = std::env::var("ORCA_AI_ENDPOINT")
        .map_err(|_| anyhow::anyhow!("set ORCA_AI_ENDPOINT (and ORCA_AI_MODEL)"))?;
    let model = std::env::var("ORCA_AI_MODEL").map_err(|_| anyhow::anyhow!("set ORCA_AI_MODEL"))?;
    let key = std::env::var("ORCA_AI_API_KEY").ok();
    let backend = OpenAiCompatibleBackend::new(endpoint, model.clone(), key);

    let scenarios: Vec<Scenario> = alert_scenarios::all()
        .into_iter()
        .filter(|s| only.is_empty() || only.iter().any(|o| *o == s.name))
        .collect();
    println!(
        "{} scenario(s), model {model}, {} prompt\n",
        scenarios.len(),
        if old { "old" } else { "new" }
    );

    let (mut total, mut answered) = (0.0, 0);
    for s in &scenarios {
        let prompt = if old {
            old_prompt(s)
        } else {
            ConversationEngine::<OpenAiCompatibleBackend>::open_prompt(
                s.service, &s.trigger, &s.ctx,
            )
        };
        println!("================ {} ({})", s.name, s.service);
        if show_prompt {
            for m in &prompt {
                println!("---- {:?}\n{}", m.role, m.content);
            }
        }
        let started = std::time::Instant::now();
        let answer = match backend.chat(&prompt).await {
            Ok(r) => r.content,
            Err(e) => {
                println!("model error: {e:#}\n");
                continue;
            }
        };
        println!(
            "---- answer ({:.1}s)\n{answer}\n",
            started.elapsed().as_secs_f64()
        );
        let score = score(s, &answer);
        total += score;
        answered += 1;
        println!("---- score {score:.2}\n");
    }
    if answered > 0 {
        println!(
            "mean score {:.2} over {answered} answered scenario(s)",
            total / answered as f64
        );
    }
    Ok(())
}

/// 0..1: expected keywords present, forbidden ones absent, sections present,
/// and the right "resolves on its own?" answer. A rough guide for comparing
/// prompts, not a verdict: read the answers.
fn score(s: &Scenario, answer: &str) -> f64 {
    let lower = answer.to_lowercase();
    let has = |k: &str| k.split('|').any(|alt| lower.contains(&alt.to_lowercase()));
    let misses: Vec<&str> = s
        .answer_keywords
        .iter()
        .copied()
        .filter(|k| !has(k))
        .collect();
    let hits = s.answer_keywords.len() - misses.len();
    let bad: Vec<&str> = s
        .answer_forbidden
        .iter()
        .copied()
        .filter(|k| lower.contains(&k.to_lowercase()))
        .collect();
    let sections = SECTIONS
        .iter()
        .filter(|h| lower.contains(&h.to_lowercase()))
        .count();
    let resolves = self_resolving(&lower);
    let resolves_ok = s.self_resolving.map(|want| resolves == Some(want));
    println!(
        "keywords {hits}/{} (missing {misses:?}), forbidden found {bad:?}, sections {sections}/{}, \
         resolves on its own: said {resolves:?}, expected {:?}",
        s.answer_keywords.len(),
        SECTIONS.len(),
        s.self_resolving
    );
    let kw = hits as f64 / s.answer_keywords.len().max(1) as f64;
    let sec = sections as f64 / SECTIONS.len() as f64;
    let base = match resolves_ok {
        Some(ok) => 0.5 * kw + 0.3 * sec + if ok { 0.2 } else { 0.0 },
        None => 0.6 * kw + 0.4 * sec,
    };
    let penalty = if bad.is_empty() { 1.0 } else { 0.5 };
    base * penalty
}

/// The first yes/no after the "resolves on its own" heading.
fn self_resolving(lower: &str) -> Option<bool> {
    let at = lower.find("resolves on its own")?;
    let tail: String = lower[at..]
        .chars()
        .skip("resolves on its own".len())
        .take(80)
        .collect();
    let yes = tail.find("yes");
    let no = tail.find("no");
    match (yes, no) {
        (Some(y), Some(n)) => Some(y < n),
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (None, None) => None,
    }
}

/// The prompt as the monitor built it before: the snapshot had counts only.
fn old_prompt(s: &Scenario) -> Vec<ChatMessage> {
    let mut ctx = s.ctx.clone();
    for svc in &mut ctx.services {
        let (name, runtime, status) = (svc.name.clone(), svc.runtime.clone(), svc.status.clone());
        let (running, desired) = (svc.replicas_running, svc.replicas_desired);
        let (errors, restarts) = (svc.error_count_1h, svc.restart_count_24h);
        *svc = orca_ai::context::ServiceSummary {
            name,
            runtime,
            replicas_running: running,
            replicas_desired: desired,
            status,
            error_count_1h: errors,
            restart_count_24h: restarts,
            ..Default::default()
        };
    }
    ctx.recent_events.clear();
    for n in &mut ctx.nodes {
        n.heartbeat_age_secs = None;
    }
    vec![
        ChatMessage {
            role: Role::System,
            content: ctx.to_system_prompt(),
        },
        ChatMessage {
            role: Role::User,
            content: format!(
                "Alert triggered for service '{}': {}\n\n\
                 Investigate this issue. Explain what's happening, the likely root cause, \
                 and suggest a fix as an `orca` command. If the issue might resolve itself, say so.",
                s.service, s.trigger
            ),
        },
    ]
}
