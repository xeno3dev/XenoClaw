import styles from './Footer.module.css'

export function Footer() {
  return (
    <footer className={styles.footer}>
      <div className={`container ${styles.inner}`}>
        <div className={styles.brand}>
          <span className={styles.logoMark}>⬡</span>
          <span className={styles.logoText}>XenoClaw</span>
          <span className={styles.license}>MIT License</span>
        </div>
        <div className={styles.links}>
          <a href="https://github.com/xeno3dev/xenoclaw" target="_blank" rel="noopener noreferrer">GitHub</a>
          <a href="#features">Features</a>
          <a href="#hosted">Managed Hosting</a>
          <a href="#quickstart">Docs</a>
        </div>
        <p className={styles.copy}>
          Built with Rust. Runs on your hardware.
        </p>
      </div>
    </footer>
  )
}
