import styles from './Providers.module.css'

const PROVIDERS = [
  'Anthropic', 'Google Gemini', 'OpenAI', 'AWS Bedrock', 'OpenRouter',
  'Together AI', 'Mistral AI', 'Fireworks AI', 'DeepSeek', 'Groq',
  'xAI', 'Perplexity', 'Cohere', 'AI21 Labs', 'Hugging Face',
  'Replicate', 'Requesty', 'Cerebras', 'SambaNova', 'Ollama',
  'vLLM', 'LM Studio', 'Qwen', 'MiniMax', 'Zhipu AI',
  'Moonshot AI', 'Baidu Qianfan',
]

export function Providers() {
  return (
    <section id="providers">
      <div className="container">
        <div className={styles.label}><span className="tag">LLM Routing</span></div>
        <h2 className={styles.title}>27 providers. Automatic failover.</h2>
        <p className={styles.sub}>
          Configure priority order. If your primary provider goes down, XenoClaw routes
          to the next one — no dropped requests, no intervention needed.
        </p>

        <div className={styles.grid}>
          {PROVIDERS.map((p) => (
            <div key={p} className={styles.chip}>{p}</div>
          ))}
        </div>

        <div className={styles.note}>
          <span className={styles.dotPulse} />
          Ollama and vLLM let you run fully local — no API key, no data leaving your server.
        </div>
      </div>
    </section>
  )
}
