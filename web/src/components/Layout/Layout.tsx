import { useState, useCallback, type ReactNode } from 'react';
import { Outlet, NavLink, useNavigate } from 'react-router-dom';
import { useAuth } from '../../hooks/useAuth';
import { useTheme } from '../../hooks/useTheme';
import styles from './Layout.module.css';

interface NavItem {
  to: string;
  label: string;
  icon: ReactNode;
}

interface NavGroup {
  label: string;
  items: NavItem[];
}

// Inline SVG keeps the bundle slim and lets icons inherit currentColor so
// the active/hover/idle states share the same color machinery as the text.
const I = {
  chat: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z" />
    </svg>
  ),
  dashboard: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="3" width="7" height="9" rx="1" />
      <rect x="14" y="3" width="7" height="5" rx="1" />
      <rect x="14" y="12" width="7" height="9" rx="1" />
      <rect x="3" y="16" width="7" height="5" rx="1" />
    </svg>
  ),
  sessions: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <line x1="3" y1="10" x2="21" y2="10" />
      <circle cx="7.5" cy="7" r="0.5" fill="currentColor" />
      <circle cx="10" cy="7" r="0.5" fill="currentColor" />
    </svg>
  ),
  tasks: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <polyline points="9 11 12 14 22 4" />
      <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
    </svg>
  ),
  memory: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 2a4 4 0 0 0-4 4v1a4 4 0 0 0-1 7 4 4 0 0 0 4 6 4 4 0 0 0 7 0 4 4 0 0 0 4-6 4 4 0 0 0-1-7V6a4 4 0 0 0-4-4 4 4 0 0 0-5 0z" />
      <line x1="12" y1="6" x2="12" y2="20" />
    </svg>
  ),
  plugins: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M9 3v4a2 2 0 0 1-2 2H3M21 3v4a2 2 0 0 1-2 2h-4M3 15h4a2 2 0 0 1 2 2v4M21 15h-4a2 2 0 0 0-2 2v4" />
      <rect x="9" y="9" width="6" height="6" rx="1" />
    </svg>
  ),
  settings: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  ),
  signOut: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
      <polyline points="16 17 21 12 16 7" />
      <line x1="21" y1="12" x2="9" y2="12" />
    </svg>
  ),
  newChat: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 5v14M5 12h14" />
    </svg>
  ),
  collapse: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <line x1="9" y1="4" x2="9" y2="20" />
    </svg>
  ),
  sun: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
    </svg>
  ),
  moon: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  ),
};

const NAV_GROUPS: NavGroup[] = [
  {
    label: 'Workspace',
    items: [
      { to: '/chat', label: 'Chat', icon: I.chat },
      { to: '/sessions', label: 'Sessions', icon: I.sessions },
      { to: '/memory', label: 'Memory', icon: I.memory },
    ],
  },
  {
    label: 'Agent',
    items: [
      { to: '/dashboard', label: 'Dashboard', icon: I.dashboard },
      { to: '/tasks', label: 'Tasks', icon: I.tasks },
    ],
  },
  {
    label: 'System',
    items: [
      { to: '/plugins', label: 'Plugins', icon: I.plugins },
      { to: '/settings', label: 'Settings', icon: I.settings },
    ],
  },
];

const COLLAPSE_KEY = 'xenoclaw_sidebar_collapsed';
/** Broadcast that the user wants a fresh chat (Chat page listens). */
export const NEW_CHAT_EVENT = 'xenoclaw:new-chat';

/**
 * Main application layout with a collapsible Claude-style sidebar.
 * - Desktop (>=768px): sidebar + content side by side; collapses to an icon rail.
 * - Mobile (<768px): hamburger toggles a sidebar overlay.
 * Supports viewports from 320px to 2560px.
 */
export function Layout() {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem(COLLAPSE_KEY) === '1';
    } catch {
      return false;
    }
  });
  const { username, logout } = useAuth();
  const { resolvedTheme, toggleTheme } = useTheme();
  const navigate = useNavigate();

  const toggleSidebar = useCallback(() => setSidebarOpen((prev) => !prev), []);
  const closeSidebar = useCallback(() => setSidebarOpen(false), []);

  const toggleCollapsed = useCallback(() => {
    setCollapsed((prev) => {
      const next = !prev;
      try {
        localStorage.setItem(COLLAPSE_KEY, next ? '1' : '0');
      } catch {
        /* ignore */
      }
      return next;
    });
  }, []);

  const startNewChat = useCallback(() => {
    closeSidebar();
    navigate('/chat');
    // Reset the conversation even when already on /chat.
    window.dispatchEvent(new CustomEvent(NEW_CHAT_EVENT));
  }, [closeSidebar, navigate]);

  const handleLogout = useCallback(() => {
    logout();
    navigate('/login');
  }, [logout, navigate]);

  const userInitial = (username ?? '?').trim().charAt(0).toUpperCase() || '?';

  return (
    <div className={`${styles.layout} ${collapsed ? styles.layoutCollapsed : ''}`}>
      {/* Mobile header with hamburger */}
      <header className={styles.mobileHeader}>
        <button
          className={styles.hamburger}
          onClick={toggleSidebar}
          aria-label={sidebarOpen ? 'Close navigation' : 'Open navigation'}
          aria-expanded={sidebarOpen}
        >
          <span className={styles.hamburgerLine} />
          <span className={styles.hamburgerLine} />
          <span className={styles.hamburgerLine} />
        </button>
        <h1 className={styles.mobileTitle}>XenoClaw</h1>
      </header>

      {/* Sidebar overlay for mobile */}
      {sidebarOpen && (
        <div className={styles.overlay} onClick={closeSidebar} aria-hidden="true" />
      )}

      {/* Sidebar navigation */}
      <aside
        className={`${styles.sidebar} ${sidebarOpen ? styles.sidebarOpen : ''} ${
          collapsed ? styles.collapsed : ''
        }`}
        aria-label="Main navigation"
      >
        <div className={styles.sidebarHeader}>
          <div className={styles.brandRow}>
            <h2 className={styles.brand}>
              <span className={styles.brandMark} aria-hidden="true" />
              <span className={styles.brandText}>XenoClaw</span>
            </h2>
            <button
              className={styles.collapseButton}
              onClick={toggleCollapsed}
              aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
              title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
            >
              {I.collapse}
            </button>
          </div>
        </div>

        <button
          className={styles.newChat}
          onClick={startNewChat}
          title="New chat"
        >
          <span className={styles.newChatIcon} aria-hidden="true">{I.newChat}</span>
          <span className={styles.newChatLabel}>New chat</span>
        </button>

        <nav className={styles.nav}>
          {NAV_GROUPS.map((group) => (
            <div key={group.label} className={styles.navGroup}>
              <span className={styles.navGroupLabel}>{group.label}</span>
              {group.items.map((item) => (
                <NavLink
                  key={item.to}
                  to={item.to}
                  title={item.label}
                  className={({ isActive }) =>
                    `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
                  }
                  onClick={closeSidebar}
                >
                  <span className={styles.navIcon}>{item.icon}</span>
                  <span className={styles.navLabel}>{item.label}</span>
                </NavLink>
              ))}
            </div>
          ))}
        </nav>

        <div className={styles.sidebarFooter}>
          <button
            className={styles.themeToggle}
            onClick={toggleTheme}
            aria-label={resolvedTheme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
            title={resolvedTheme === 'dark' ? 'Light mode' : 'Dark mode'}
          >
            <span className={styles.themeIcon} aria-hidden="true">
              {resolvedTheme === 'dark' ? I.sun : I.moon}
            </span>
            <span className={styles.themeLabel}>
              {resolvedTheme === 'dark' ? 'Light mode' : 'Dark mode'}
            </span>
          </button>

          <div className={styles.userBlock}>
            <span className={styles.userAvatar} aria-hidden="true">{userInitial}</span>
            <span className={styles.username} title={username ?? ''}>{username}</span>
            <button
              className={styles.logoutButton}
              onClick={handleLogout}
              aria-label="Sign out"
              title="Sign out"
            >
              <span className={styles.logoutIcon} aria-hidden="true">{I.signOut}</span>
            </button>
          </div>
        </div>
      </aside>

      {/* Main content area */}
      <main className={styles.main}>
        <Outlet />
      </main>
    </div>
  );
}
