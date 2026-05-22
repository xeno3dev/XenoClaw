import styles from './Features.module.css'

const FEATURES = [
  {
    icon: '⚡',
    title: 'Always-On Runtime',
    desc: 'Process supervisor with auto-restart under 10 seconds. Health checks every 15s. SIGHUP hot-reload — zero downtime config changes.',
  },
  {
    icon: '🔀',
    title: 'Dual Agent Mode',
    desc: 'Switch between General (chat, tasks, knowledge) and Coding (file ops, shell, git, LSP, diffs) at runtime. No restart needed.',
  },
  {
    icon: '🌐',
    title: '27-Provider Failover',
    desc: 'Anthropic, OpenAI, Gemini, Ollama, Groq, Mistral, DeepSeek, and 20 more. Priority-based routing with automatic failover. No dropped requests.',
  },
  {
    icon: '💬',
    title: 'Messaging Bridge',
    desc: 'Chat from Telegram, Discord, or WhatsApp. Messages hit the same agent core, same tools, same memory — wherever you are.',
  },
  {
    icon: '🧩',
    title: 'WASM Plugin System',
    desc: 'Hot-reload sandboxed plugins. Register custom tools, event handlers, memory scopes. A crashed plugin never takes down the agent.',
  },
  {
    icon: '🔒',
    title: 'Built-In Security',
    desc: 'Filesystem & network sandboxing, RBAC roles, brute-force protection, resource limits, SHA-256 hashed keys, full audit trail.',
  },
  {
    icon: '🔌',
    title: 'MCP Server',
    desc: 'Expose all tools via Model Context Protocol. Let Claude Code or GitHub Copilot use XenoClaw\'s file ops, memory, and task scheduler.',
  },
  {
    icon: '📊',
    title: 'Observability',
    desc: 'Prometheus metrics endpoint, JSON structured logging with tracing, configurable alert rules, log rotation and retention policies.',
  },
]

export function Features() {
  return (
    <section id="features">
      <div className="container">
        <div className="section-label">
          <span className="tag">Features</span>
        </div>
        <h2 className={styles.title}>Everything you need.<br />Nothing you don't.</h2>
        <p className={styles.sub}>
          A complete AI agent stack you own end-to-end. Built in Rust. 12 workspace crates. ~512 MB idle.
        </p>

        <div className={styles.grid}>
          {FEATURES.map((f) => (
            <div key={f.title} className={styles.card}>
              <span className={styles.icon}>{f.icon}</span>
              <h3 className={styles.cardTitle}>{f.title}</h3>
              <p className={styles.cardDesc}>{f.desc}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}
