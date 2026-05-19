import { useState, useCallback } from 'react';
import { Outlet, NavLink } from 'react-router-dom';
import styles from './Layout.module.css';

/**
 * Main application layout with responsive sidebar.
 * - Desktop (>=768px): Sidebar + main content side by side
 * - Mobile (<768px): Hamburger menu toggles sidebar overlay
 * Supports viewports from 320px to 2560px.
 */
export function Layout() {
  const [sidebarOpen, setSidebarOpen] = useState(false);

  const toggleSidebar = useCallback(() => {
    setSidebarOpen((prev) => !prev);
  }, []);

  const closeSidebar = useCallback(() => {
    setSidebarOpen(false);
  }, []);

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
        </nav>
      </aside>

      {/* Main content area */}
      <main className={styles.main}>
        <Outlet />
      </main>
    </div>
  );
}
