//! Every alert scenario's key evidence must reach the model's prompt. This
//! guards the context plumbing without calling a model; `examples/alert_eval`
//! judges the answers against a real one.

#[path = "fixtures/alert_scenarios.rs"]
mod alert_scenarios;

use orca_ai::backend::OpenAiCompatibleBackend;
use orca_ai::backend::Role;
use orca_ai::conversation::ConversationEngine;

#[test]
fn each_scenario_puts_its_evidence_in_the_prompt() {
    for s in alert_scenarios::all() {
        let prompt = ConversationEngine::<OpenAiCompatibleBackend>::open_prompt(
            s.service, &s.trigger, &s.ctx,
        );
        let text: String = prompt
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for needle in s.evidence {
            assert!(
                text.contains(needle),
                "scenario {}: prompt is missing {needle:?}\n---\n{text}",
                s.name
            );
        }
        assert!(matches!(prompt[0].role, Role::System));
        assert!(prompt[1].content.contains("**Likely cause**"), "{}", s.name);
    }
}

#[test]
fn healthy_services_are_one_list_not_a_section_each() {
    let s = alert_scenarios::all().remove(0);
    let prompt = s.ctx.alert_prompt(s.service);
    assert!(prompt.contains("## Healthy services (9)"));
    assert!(prompt.contains("gitea, keycloak"));
    assert_eq!(prompt.matches("## Affected service").count(), 1);
}
