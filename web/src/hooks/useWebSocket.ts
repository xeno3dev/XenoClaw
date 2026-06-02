import { useCallback, useEffect, useRef, useState } from 'react';

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'reconnecting';

export interface WebSocketMessage {
  type: string;
  /**
   * Some server messages (the events stream) nest their data under `payload`;
   * the chat stream sends its fields flat alongside `type`. The index signature
   * lets callers read either shape.
   */
  payload?: unknown;
  [key: string]: unknown;
}

export interface UseWebSocketOptions {
  /** Authentication token for the WebSocket connection */
  token: string;
  /** WebSocket endpoint path (e.g., '/api/v1/ws/chat/session-id' or '/api/v1/ws/events') */
  endpoint: string;
  /** Callback when a message is received */
  onMessage?: (message: WebSocketMessage) => void;
  /** Callback when connection status changes */
  onStatusChange?: (status: ConnectionStatus) => void;
  /** Whether to automatically connect on mount (default: true) */
  autoConnect?: boolean;
}

interface UseWebSocketReturn {
  /** Current connection status */
  status: ConnectionStatus;
  /** Send a message through the WebSocket */
  send: (message: WebSocketMessage) => void;
  /** Manually connect */
  connect: () => void;
  /** Manually disconnect */
  disconnect: () => void;
}

/**
 * WebSocket hook for real-time communication with the XenoClaw backend.
 * Handles authentication via token query parameter and implements
 * auto-reconnect on disconnect (retry within 10s, then every 30s).
 */
export function useWebSocket(options: UseWebSocketOptions): UseWebSocketReturn {
  const { token, endpoint, onMessage, onStatusChange, autoConnect = true } = options;

  const [status, setStatus] = useState<ConnectionStatus>('disconnected');
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reconnectAttemptRef = useRef(0);
  const intentionalCloseRef = useRef(false);

  const updateStatus = useCallback((newStatus: ConnectionStatus) => {
    setStatus(newStatus);
    onStatusChange?.(newStatus);
  }, [onStatusChange]);

  const clearReconnectTimer = useCallback(() => {
    if (reconnectTimerRef.current !== null) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
  }, []);

  const getReconnectDelay = useCallback((): number => {
    // First attempt: retry within 10s, subsequent attempts: every 30s
    return reconnectAttemptRef.current === 0 ? 10_000 : 30_000;
  }, []);

  const buildWsUrl = useCallback((): string => {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const host = window.location.host;
    const url = `${protocol}//${host}${endpoint}?token=${encodeURIComponent(token)}`;
    return url;
  }, [endpoint, token]);

  const connect = useCallback(() => {
    // Close existing connection if any
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    intentionalCloseRef.current = false;
    clearReconnectTimer();

    const url = buildWsUrl();
    updateStatus('connecting');

    const ws = new WebSocket(url);

    ws.onopen = () => {
      reconnectAttemptRef.current = 0;
      updateStatus('connected');
    };

    ws.onmessage = (event: MessageEvent) => {
      try {
        const message = JSON.parse(event.data as string) as WebSocketMessage;
        onMessage?.(message);
      } catch {
        // If the message isn't valid JSON, wrap it
        onMessage?.({ type: 'raw', payload: event.data });
      }
    };

    ws.onclose = () => {
      wsRef.current = null;

      if (intentionalCloseRef.current) {
        updateStatus('disconnected');
        return;
      }

      // Auto-reconnect
      updateStatus('reconnecting');
      const delay = getReconnectDelay();
      reconnectAttemptRef.current += 1;

      reconnectTimerRef.current = setTimeout(() => {
        connect();
      }, delay);
    };

    ws.onerror = () => {
      // The onclose handler will fire after onerror, handling reconnection
    };

    wsRef.current = ws;
  }, [buildWsUrl, clearReconnectTimer, getReconnectDelay, onMessage, updateStatus]);

  const disconnect = useCallback(() => {
    intentionalCloseRef.current = true;
    clearReconnectTimer();
    reconnectAttemptRef.current = 0;

    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    updateStatus('disconnected');
  }, [clearReconnectTimer, updateStatus]);

  const send = useCallback((message: WebSocketMessage) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(message));
    }
  }, []);

  // Auto-connect on mount if enabled
  useEffect(() => {
    if (autoConnect && token) {
      connect();
    }

    return () => {
      intentionalCloseRef.current = true;
      clearReconnectTimer();
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token, endpoint]);

  return { status, send, connect, disconnect };
}
