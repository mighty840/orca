---
layout: home

hero:
  name: Orca
  text: Container + Wasm Orchestrator
  tagline: Fills the gap between Coolify and Kubernetes
  image:
    src: /logo.svg
    alt: Orca
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: View on GitHub
      link: https://github.com/mighty840/orca

features:
  - icon: "\U0001F4E6"
    title: Single Binary
    details: One static executable is the agent, control plane, CLI, and proxy. scp it to a server and run.
  - icon: "\U0001F512"
    title: Auto-TLS
    details: Built-in ACME/Let's Encrypt. Certificates are provisioned and renewed automatically for every domain.
  - icon: "\U0001F504"
    title: Self-Healing
    details: Watchdog restarts crashed containers in ~30s. Health checks, stale route cleanup, and agent reconnection are all automatic.
  - icon: "\U0001F916"
    title: AI Ops
    details: "Ask your cluster questions in plain English: orca ask \"why is the API slow?\" — powered by any OpenAI-compatible LLM."
  - icon: "\U0001F310"
    title: Multi-Node
    details: Raft consensus with embedded redb storage. No etcd. Bin-packing scheduler with GPU awareness across 2-20 nodes.
  - icon: "\U0001F4C1"
    title: Project Isolation
    details: Directory-based namespaces with per-project secrets, networks, and service grouping. Config fits on one screen.
---

<div style="text-align: center; margin-top: 2rem;">

```
Docker Compose ──> Coolify ──> Orca ──> Kubernetes
   (1 node)        (1 node)   (2-20)     (20-10k)
```

</div>

## Quick Install

```bash
cargo install mallorca
sudo setcap 'cap_net_bind_service=+ep' $(which orca)
orca server
```
