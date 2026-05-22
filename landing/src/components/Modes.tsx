import styles from './Modes.module.css'

const GENERAL = [
  'LLM chat with memory across sessions',
  'Cron + event-driven task scheduler',
  'Knowledge store with vector search',
  'WASM plugin tools',
  'Telegram / Discord / WhatsApp bridge',
  'REST + WebSocket API',
  'Prometheus metrics & structured logs',
]

const CODING = [
  'Everything in General mode',
  'File read / write with undo history',
  'Sandboxed shell execution',
  'Git operations (commit, diff, log)',
  'LSP diagnostics integration',
  'Visual diff rendering',
  'MCP server for Claude Code & Copilot',
]

export function Modes() {
  return (
    <section>
      <div className="container">
        <div className={styles.label}><span className="tag">Dual Mode</span></div>
        <h2 className={styles.title}>One runtime. Two modes.<br />Hot-swap at any time.</h2>
        <p className={styles.sub}>
          Start as a general assistant, flip into coding mode for a task, flip back — no restarts, no context loss.
        </p>

        <div className={styles.grid}>
          <ModeCard
            name="General"
            tagline="Task automation, scheduling, knowledge management"
            items={GENERAL}
            accent="amber"
          />
          <ModeCard
            name="Coding"
            tagline="Full-stack coding agent on your VPS"
            items={CODING}
            accent="red"
          />
        </div>
      </div>
    </section>
  )
}

function ModeCard({
  name,
  tagline,
  items,
  accent,
}: {
  name: string
  tagline: string
  items: string[]
  accent: 'red' | 'amber'
}) {
  return (
    <div className={`${styles.card} ${styles[accent]}`}>
      <div className={styles.cardHeader}>
        <span className={`${styles.modeBadge} ${styles[`modeBadge_${accent}`]}`}>{name}</span>
        <p className={styles.cardTagline}>{tagline}</p>
      </div>
      <ul className={styles.list}>
        {items.map((item) => (
          <li key={item} className={styles.item}>
            <span className={styles.check}>✓</span>
            {item}
          </li>
        ))}
      </ul>
    </div>
  )
}
