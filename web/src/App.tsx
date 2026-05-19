import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import { AuthProvider } from './hooks/useAuth';
import { ProtectedRoute } from './components/ProtectedRoute';
import { Layout } from './components/Layout/Layout';
import { Login } from './pages/Login';
import { Chat } from './pages/Chat';
import { Dashboard } from './pages/Dashboard';
import { Sessions } from './pages/Sessions';
import { Settings } from './pages/Settings';
import { Plugins } from './pages/Plugins';
import { Tasks } from './pages/Tasks';
import { Memory } from './pages/Memory';

function App() {
  return (
    <AuthProvider>
      <BrowserRouter>
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
      </BrowserRouter>
    </AuthProvider>
  );
}

export default App;
