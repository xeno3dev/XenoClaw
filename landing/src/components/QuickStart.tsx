import styles from './QuickStart.module.css'

const STEPS = [
  {
    num: '01',
    title: 'Install',
    code: `# Requires Rust ≥ 1.75
./install.sh
# or: cargo install --path crates/xenoclaw`,
  },
  {
    num: '02',
    title: 'Configure',
    code: `# Interactive setup wizard
xenoclaw -s
# Walks you through: provider keys,
# API config, workspace init (8 steps)`,
  },
  {
    num: '03',
    title: 'Run',
    code: `# Start the agent runtime
xenoclaw

# Binds to 0.0.0.0:9090 by default
# Web UI at http://localhost:9090`,
  },
  {
    num: '04',
    title: 'Connect Claude Code',
    code: `# Expose tools via MCP
claude mcp add xenoclaw -- xenoclaw mcp

# Now Claude Code can use XenoClaw's
# file ops, memory, and task scheduler`,
  },
]

export function QuickStart() {
  return (
    <section id="quickstart">
      <div className="container">
        <div className={styles.label}><span className="tag">Quick Start</span></div>
        <h2 className={styles.title}>Up and running in 5 minutes.</h2>
        <p className={styles.sub}>
          One binary. One config file. Deploy to any Linux VPS or bare-metal machine.
        </p>

        <div className={styles.steps}>
          {STEPS.map((step) => (
            <div key={step.num} className={styles.step}>
              <div className={styles.stepNum}>{step.num}</div>
              <div className={styles.stepBody}>
                <h3 className={styles.stepTitle}>{step.title}</h3>
                <pre className={styles.code}><code>{step.code}</code></pre>
              </div>
            </div>
          ))}
        </div>

        <div className={styles.deploy}>
          <h3 className={styles.deployTitle}>Deploy options</h3>
          <div className={styles.deployGrid}>
            <DeployCard
              icon="🖥"
              title="systemd"
              code={`sudo cp xenoclaw /opt/xenoclaw/bin/
sudo cp deploy/xenoclaw-agent.service \\
  /etc/systemd/system/
sudo systemctl enable --now xenoclaw-agent`}
            />
            <DeployCard
              icon="🐳"
              title="Docker Compose"
              code={`docker compose \\
  -f deploy/docker-compose.yml \\
  up -d`}
            />
          </div>
        </div>
      </div>
    </section>
  )
}

function DeployCard({ icon, title, code }: { icon: string; title: string; code: string }) {
  return (
    <div className={styles.deployCard}>
      <div className={styles.deployCardHeader}>
        <span>{icon}</span>
        <span className={styles.deployCardTitle}>{title}</span>
      </div>
      <pre className={styles.deployCode}><code>{code}</code></pre>
    </div>
  )
}
