import { useState, useRef, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '../../lib/tauri';
import { useServers, useConnection, type LiveStatus } from '../../hooks/useServer';
import { useTheme } from '../../hooks/useTheme';
import styles from './Titlebar.module.css';

const STATUS_LABEL: Record<LiveStatus, string> = {
  connected: 'Connected',
  connecting: 'Connecting…',
  offline: 'Offline',
};

/**
 * Native window chrome for the desktop app: drag region, app brand, active
 * server switcher + live connection indicator, theme toggle, and window
 * controls. Rendered only inside the Tauri shell.
 */
export function Titlebar() {
  const { profiles, activeId, active, setActive } = useServers();
  const status = useConnection();
  const { resolved, setTheme } = useTheme();
  const navigate = useNavigate();
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const onClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', onClick);
    return () => document.removeEventListener('mousedown', onClick);
  }, [menuOpen]);

  const win = useCallback((cmd: string) => {
    void invoke(cmd).catch(() => {});
  }, []);

  return (
    <div className={styles.titlebar} data-tauri-drag-region>
      <div className={styles.left} data-tauri-drag-region>
        <span className={styles.brandDot} aria-hidden="true" />
        <span className={styles.brand} data-tauri-drag-region>
          XenoClaw
        </span>
      </div>

      <div className={styles.center}>
        <div className={styles.serverSwitcher} ref={menuRef}>
          <button
            className={styles.serverButton}
            onClick={() => setMenuOpen((o) => !o)}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            type="button"
          >
            <span className={`${styles.statusDot} ${styles[`dot_${status}`]}`} aria-hidden="true" />
            <span className={styles.serverName}>{active?.name ?? 'No server'}</span>
            <svg viewBox="0 0 24 24" className={styles.chevron} aria-hidden="true">
              <polyline points="6 9 12 15 18 9" fill="none" stroke="currentColor" strokeWidth="2" />
            </svg>
          </button>

          {menuOpen && (
            <div className={styles.menu} role="menu">
              <div className={styles.menuLabel}>Connection: {STATUS_LABEL[status]}</div>
              {profiles.map((p) => (
                <button
                  key={p.id}
                  className={`${styles.menuItem} ${p.id === activeId ? styles.menuItemActive : ''}`}
                  onClick={() => {
                    setActive(p.id);
                    setMenuOpen(false);
                  }}
                  role="menuitem"
                  type="button"
                >
                  <span className={styles.menuItemName}>{p.name}</span>
                  <span className={styles.menuItemUrl}>
                    {p.kind === 'local' ? p.url || 'not started' : p.url || '—'}
                  </span>
                </button>
              ))}
              <div className={styles.menuDivider} />
              <button
                className={styles.menuItem}
                onClick={() => {
                  setMenuOpen(false);
                  navigate('/settings');
                }}
                role="menuitem"
                type="button"
              >
                <span className={styles.menuItemName}>Manage servers…</span>
              </button>
            </div>
          )}
        </div>
      </div>

      <div className={styles.right}>
        <button
          className={styles.iconButton}
          onClick={() => setTheme(resolved === 'dark' ? 'light' : 'dark')}
          title={resolved === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
          aria-label="Toggle theme"
          type="button"
        >
          {resolved === 'dark' ? (
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" strokeWidth="2" />
              <path
                d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
              />
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinejoin="round"
              />
            </svg>
          )}
        </button>

        <div className={styles.windowControls}>
          <button
            className={styles.winButton}
            onClick={() => win('win_minimize')}
            aria-label="Minimize"
            type="button"
          >
            <svg viewBox="0 0 12 12" aria-hidden="true">
              <line x1="2" y1="6" x2="10" y2="6" stroke="currentColor" strokeWidth="1.2" />
            </svg>
          </button>
          <button
            className={styles.winButton}
            onClick={() => win('win_toggle_maximize')}
            aria-label="Maximize"
            type="button"
          >
            <svg viewBox="0 0 12 12" aria-hidden="true">
              <rect x="2.5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" strokeWidth="1.2" />
            </svg>
          </button>
          <button
            className={`${styles.winButton} ${styles.winClose}`}
            onClick={() => win('win_close')}
            aria-label="Close"
            type="button"
          >
            <svg viewBox="0 0 12 12" aria-hidden="true">
              <line x1="3" y1="3" x2="9" y2="9" stroke="currentColor" strokeWidth="1.2" />
              <line x1="9" y1="3" x2="3" y2="9" stroke="currentColor" strokeWidth="1.2" />
            </svg>
          </button>
        </div>
      </div>
    </div>
  );
}
