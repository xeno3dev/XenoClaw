import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import { AuthProvider } from './hooks/useAuth';
import { ProtectedRoute } from './components/ProtectedRoute';
import { Layout } from './components/Layout/Layout';
import { Titlebar } from './components/Titlebar/Titlebar';
import { useTheme } from './hooks/useTheme';
import { isTauri } from './lib/tauri';
import { Login } from './pages/Login';
import { Chat } from './pages/Chat';
import { Dashboard } from './pages/Dashboard';
import { Sessions } from './pages/Sessions';
import { Settings } from './pages/Settings';
import { Plugins } from './pages/Plugins';
import { Tasks } from './pages/Tasks';
import { Memory } from './pages/Memory';

/**
 * App shell. In the desktop build it renders the native titlebar above the
 * routed content; on the web it renders the routes directly. `useTheme` keeps
 * <html data-theme> in sync with the saved preference / OS setting.
 */
function AppShell() {
  useTheme();
  const desktop = isTauri();

  const routes = (
    <Routes>
      <Route path="/login" element={<Login />} />
      <Route
        element={
          <ProtectedRoute>
            <Layout />
          </ProtectedRoute>
        }
      >
        <Route path="/chat" element={<Chat />} />
        <Route path="/dashboard" element={<Dashboard />} />
        <Route path="/sessions" element={<Sessions />} />
        <Route path="/settings" element={<Settings />} />
        <Route path="/plugins" element={<Plugins />} />
        <Route path="/tasks" element={<Tasks />} />
        <Route path="/memory" element={<Memory />} />
        <Route path="/" element={<Navigate to="/chat" replace />} />
      </Route>
    </Routes>
  );

  if (!desktop) return routes;

  return (
    <div className="desktop-shell">
      <Titlebar />
      <div className="app-body">{routes}</div>
    </div>
  );
}

function App() {
  return (
    <AuthProvider>
      <BrowserRouter>
        <AppShell />
      </BrowserRouter>
    </AuthProvider>
  );
}

export default App;
