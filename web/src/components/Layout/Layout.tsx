import { useState, useCallback } from 'react';
import { Outlet, NavLink, useNavigate } from 'react-router-dom';
import { useAuth } from '../../hooks/useAuth';
import styles from './Layout.module.css';

/**
 * Main application layout with responsive sidebar.
 * - Desktop (>=768px): Sidebar + main content side by side
 * - Mobile (<768px): Hamburger menu toggles sidebar overlay
 * Supports viewports from 320px to 2560px.
 */
export function Layout() {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const { username, logout } = useAuth();
  const navigate = useNavigate();

  const toggleSidebar = useCallback(() => {
    setSidebarOpen((prev) => !prev);
  }, []);

  const closeSidebar = useCallback(() => {
    setSidebarOpen(false);
  }, []);

  const handleLogout = useCallback(() => {
    logout();
    navigate('/login');
  }, [logout, navigate]);

  return (
    <div className={styles.layout}>
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
        <div
          className={styles.overlay}
          onClick={closeSidebar}
          aria-hidden="true"
        />
      )}

      {/* Sidebar navigation */}
      <aside
        className={`${styles.sidebar} ${sidebarOpen ? styles.sidebarOpen : ''}`}
        aria-label="Main navigation"
      >
        <div className={styles.sidebarHeader}>
          <h2 className={styles.brand}>XenoClaw</h2>
        </div>
        <nav className={styles.nav}>
          <NavLink
            to="/chat"
            className={({ isActive }) =>
              `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
            }
            onClick={closeSidebar}
          >
            Chat
          </NavLink>
          <NavLink
            to="/dashboard"
            className={({ isActive }) =>
              `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
            }
            onClick={closeSidebar}
          >
            Dashboard
          </NavLink>
          <NavLink
            to="/sessions"
            className={({ isActive }) =>
              `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
            }
            onClick={closeSidebar}
          >
            Sessions
          </NavLink>
          <NavLink
            to="/tasks"
            className={({ isActive }) =>
              `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
            }
            onClick={closeSidebar}
          >
            Tasks
          </NavLink>
          <NavLink
            to="/memory"
            className={({ isActive }) =>
              `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
            }
            onClick={closeSidebar}
          >
            Memory
          </NavLink>
          <NavLink
            to="/plugins"
            className={({ isActive }) =>
              `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
            }
            onClick={closeSidebar}
          >
            Plugins
          </NavLink>
          <NavLink
            to="/settings"
            className={({ isActive }) =>
              `${styles.navLink} ${isActive ? styles.navLinkActive : ''}`
            }
            onClick={closeSidebar}
          >
            Settings
          </NavLink>
        </nav>
        <div className={styles.sidebarFooter}>
          <span className={styles.username}>{username}</span>
          <button className={styles.logoutButton} onClick={handleLogout}>
            Sign out
          </button>
        </div>
      </aside>

      {/* Main content area */}
      <main className={styles.main}>
        <Outlet />
      </main>
    </div>
  );
}
