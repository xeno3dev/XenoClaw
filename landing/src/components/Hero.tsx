import styles from './Hero.module.css'

export function Hero() {
  return (
    <section className={styles.hero}>
      <div className={`container ${styles.inner}`}>
        <div className={styles.badge}>
          <span className="tag">Self-Hosted · Always-On · Open Source</span>
        </div>

        <h1 className={styles.headline}>
          Your AI agent runtime.<br />
          <span className={styles.accent}>Your infrastructure.</span>
        </h1>

        <p className={styles.sub}>
          XenoClaw is a modular, always-on AI agent system that runs on your VPS or bare-metal server.
          Dual-mode (General + Coding), 27 LLM providers with auto-failover, messaging bridge,
          WASM plugins — all yours.
        </p>

        <div className={styles.actions}>
          <a href="#hosted" className="btn-primary">
            Get a Managed Instance →
          </a>
          <a href="#quickstart" className="btn-ghost">
            Self-Host in 5 Minutes
          </a>
        </div>

        <div className={styles.stats}>
          <Stat value="27" label="LLM Providers" />
          <div className={styles.divider} />
          <Stat value="12" label="Rust Crates" />
          <div className={styles.divider} />
          <Stat value="< 10s" label="Auto-Restart" />
          <div className={styles.divider} />
          <Stat value="MIT" label="License" />
        </div>

        <div className={styles.terminal}>
          <div className={styles.termBar}>
            <span className={styles.dot} style={{ background: '#ff5f57' }} />
            <span className={styles.dot} style={{ background: '#febc2e' }} />
            <span className={styles.dot} style={{ background: '#28c840' }} />
            <span className={styles.termTitle}>xenoclaw</span>
          </div>
          <pre className={styles.termBody}>{`$ xenoclaw -s
  ██╗  ██╗███████╗███╗   ██╗ ██████╗  ██████╗██╗      █████╗ ██╗    ██╗
  ╚██╗██╔╝██╔════╝████╗  ██║██╔═══██╗██╔════╝██║     ██╔══██╗██║    ██║
   ╚███╔╝ █████╗  ██╔██╗ ██║██║   ██║██║     ██║     ███████║██║ █╗ ██║
   ██╔██╗ ██╔══╝  ██║╚████║ ██║   ██║██║     ██║     ██╔══██║██║███╗██║
  ██╔╝ ██╗███████╗██║ ╚███║ ╚██████╔╝╚██████╗███████╗██║  ██║╚███╔███╔╝
  ╚═╝  ╚═╝╚══════╝╚═╝  ╚══╝ ╚═════╝  ╚═════╝╚══════╝╚═╝  ╚═╝ ╚══╝╚══╝

  <span style="color:#50dc64">✓</span> Config written → ~/.xenoclaw/config.toml
  <span style="color:#50dc64">✓</span> API key generated → sk-xeno-••••••••••••••••
  <span style="color:#50dc64">✓</span> Workspace initialized
  <span style="color:#ffb000">→</span> Run <span style="color:#ff6060">xenoclaw</span> to start the agent runtime`}</pre>
        </div>
      </div>
    </section>
  )
}

function Stat({ value, label }: { value: string; label: string }) {
  return (
    <div className={styles.stat}>
      <span className={styles.statValue}>{value}</span>
      <span className={styles.statLabel}>{label}</span>
    </div>
  )
}
