import { useState } from 'react'
import styles from './GetHosted.module.css'

const PLANS = [
  {
    name: 'Starter',
    price: '$29',
    period: '/mo',
    desc: 'Everything included. No API costs billed separately.',
    competitorNote: 'Competitors charge $20–$24 + LLM API on top',
    features: [
      '1 XenoClaw instance',
      '2 vCPU · 4 GB RAM · 50 GB SSD',
      'All 27 LLM providers with failover',
      'General + Coding agent modes',
      'Telegram + Discord bridge',
      'Automatic updates',
      'Email support',
    ],
    cta: 'Start Free Trial',
    highlight: false,
  },
  {
    name: 'Pro',
    price: '$69',
    period: '/mo',
    desc: 'More headroom. Full feature set unlocked.',
    competitorNote: 'OpenClaw Cloud charges $59 with no coding agent',
    features: [
      '1 XenoClaw instance',
      '4 vCPU · 8 GB RAM · 200 GB SSD',
      'All 27 LLM providers with failover',
      'General + Coding agent modes',
      'All messaging bridges (+ WhatsApp)',
      'WASM plugin system',
      'Daily backups',
      'MCP server for Claude Code',
      'Priority email support',
    ],
    cta: 'Start Free Trial',
    highlight: true,
  },
  {
    name: 'Team',
    price: '$249',
    period: '/mo',
    desc: 'Multi-instance. No other managed host offers this.',
    competitorNote: 'Unique in the category — no competitor offers multi-instance',
    features: [
      'Up to 3 XenoClaw instances',
      '8 vCPU · 16 GB RAM each · 500 GB SSD',
      'All 27 LLM providers with failover',
      'General + Coding agent modes',
      'All messaging bridges',
      'WASM plugin system + custom deploys',
      'Daily backups + point-in-time restore',
      'MCP server for Claude Code & Copilot',
      'Slack + priority support',
    ],
    cta: 'Contact Us',
    highlight: false,
  },
]

export function GetHosted() {
  const [submitted, setSubmitted] = useState(false)
  const [email, setEmail] = useState('')
  const [selectedPlan, setSelectedPlan] = useState('Pro')

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!email) return
    setSubmitted(true)
  }

  return (
    <section id="hosted">
      <div className="container">
        <div className={styles.label}><span className="tag">Managed Hosting</span></div>
        <h2 className={styles.title}>Zero ops. Full power.<br />No hidden API costs.</h2>
        <p className={styles.sub}>
          We run XenoClaw for you — deployed, monitored, updated, backed up.
          Infrastructure is included. Unlike competitors, you won't get a surprise
          LLM API bill on top.
        </p>

        <div className={styles.plans}>
          {PLANS.map((plan) => (
            <div
              key={plan.name}
              className={`${styles.plan} ${plan.highlight ? styles.planHighlight : ''}`}
            >
              {plan.highlight && (
                <div className={styles.popularBadge}>Most Popular</div>
              )}
              <div className={styles.planHeader}>
                <h3 className={styles.planName}>{plan.name}</h3>
                <div className={styles.planPrice}>
                  <span className={styles.price}>{plan.price}</span>
                  <span className={styles.period}>{plan.period}</span>
                </div>
                <p className={styles.planDesc}>{plan.desc}</p>
                <p className={styles.competitorNote}>↳ {plan.competitorNote}</p>
              </div>
              <ul className={styles.planFeatures}>
                {plan.features.map((f) => (
                  <li key={f} className={styles.planFeature}>
                    <span className={styles.featureCheck}>✓</span>
                    {f}
                  </li>
                ))}
              </ul>
              <button
                className={plan.highlight ? `btn-primary ${styles.planBtn}` : `btn-ghost ${styles.planBtn}`}
                onClick={() => {
                  setSelectedPlan(plan.name)
                  document.getElementById('waitlist-form')?.scrollIntoView({ behavior: 'smooth', block: 'center' })
                }}
              >
                {plan.cta}
              </button>
            </div>
          ))}
        </div>

        <div id="waitlist-form" className={styles.form}>
          {submitted ? (
            <div className={styles.success}>
              <span className={styles.successIcon}>✓</span>
              <h3>You're on the list.</h3>
              <p>We'll reach out to <strong>{email}</strong> with next steps for your {selectedPlan} instance.</p>
            </div>
          ) : (
            <>
              <h3 className={styles.formTitle}>Join the waitlist</h3>
              <p className={styles.formSub}>Hosted instances are in early access. Drop your email and we'll be in touch.</p>
              <form className={styles.formRow} onSubmit={handleSubmit}>
                <input
                  className={styles.input}
                  type="email"
                  placeholder="you@example.com"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  required
                />
                <select
                  className={styles.select}
                  value={selectedPlan}
                  onChange={(e) => setSelectedPlan(e.target.value)}
                >
                  {PLANS.map((p) => (
                    <option key={p.name} value={p.name}>{p.name} — {p.price}/mo</option>
                  ))}
                </select>
                <button type="submit" className="btn-primary">
                  Request Access →
                </button>
              </form>
              <p className={styles.privacy}>No spam. No credit card required to join.</p>
            </>
          )}
        </div>
      </div>
    </section>
  )
}
