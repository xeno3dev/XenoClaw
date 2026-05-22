import { useState, useCallback, useRef, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import { useWebSocket, type ConnectionStatus, type WebSocketMessage } from '../hooks/useWebSocket';
import styles from './Chat.module.css';

/** Unique ID generator for messages */
let messageIdCounter = 0;
function generateId(): string {
  return `msg-${Date.now()}-${++messageIdCounter}`;
}

interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'error';
  content: string;
  timestamp: Date;
  /** Optional diff image URL or inline SVG data */
  diffImage?: string;
  /** Whether this message is still being streamed */
  streaming?: boolean;
}

// Session ID — in a real app this would come from auth/routing
const SESSION_ID = 'default-session';

/**
 * Chat page — real-time chat interface with WebSocket streaming.
 * Implements Requirements 7.1, 7.4, 7.5, 16.4
 */
export function Chat() {
  const { token } = useAuth();
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [inputValue, setInputValue] = useState('');
  const [isWaitingForResponse, setIsWaitingForResponse] = useState(false);

  const messageListRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const streamingMessageRef = useRef<string | null>(null);

  // Auto-scroll to bottom when new messages arrive
  const scrollToBottom = useCallback(() => {
    if (messageListRef.current) {
      messageListRef.current.scrollTop = messageListRef.current.scrollHeight;
    }
  }, []);

  useEffect(() => {
    scrollToBottom();
  }, [messages, scrollToBottom]);

  // Handle incoming WebSocket messages
  const handleWsMessage = useCallback((wsMessage: WebSocketMessage) => {
    const { type, payload } = wsMessage;

    switch (type) {
      case 'token': {
        // Accumulate streamed tokens into the current assistant response
        const token = (payload as { content?: string })?.content ?? String(payload);
        setMessages((prev) => {
          const streamingId = streamingMessageRef.current;
          if (!streamingId) {
            // Start a new streaming message
            const newId = generateId();
            streamingMessageRef.current = newId;
            return [
              ...prev,
              {
                id: newId,
                role: 'assistant',
                content: token,
                timestamp: new Date(),
                streaming: true,
              },
            ];
          }
          // Append to existing streaming message
          return prev.map((msg) =>
            msg.id === streamingId
              ? { ...msg, content: msg.content + token }
              : msg
          );
        });
        break;
      }

      case 'done': {
        // Finalize the streaming message
        const streamingId = streamingMessageRef.current;
        if (streamingId) {
          setMessages((prev) =>
            prev.map((msg) =>
              msg.id === streamingId ? { ...msg, streaming: false } : msg
            )
          );
        }
        streamingMessageRef.current = null;
        setIsWaitingForResponse(false);
        break;
      }

      case 'error': {
        // Display error inline
        const errorContent =
          (payload as { message?: string })?.message ??
          (payload as { error?: string })?.error ??
          'An error occurred';
        streamingMessageRef.current = null;
        setIsWaitingForResponse(false);
        setMessages((prev) => [
          ...prev,
          {
            id: generateId(),
            role: 'error',
            content: errorContent,
            timestamp: new Date(),
          },
        ]);
        break;
      }

      case 'diff_image': {
        // Diff image from coding module — attach to current or create new message
        const imageData = (payload as { url?: string; svg?: string });
        const diffImage = imageData.url || imageData.svg || '';
        const streamingId = streamingMessageRef.current;

        if (streamingId) {
          // Attach to current streaming message
          setMessages((prev) =>
            prev.map((msg) =>
              msg.id === streamingId ? { ...msg, diffImage } : msg
            )
          );
        } else {
          // Create a standalone diff image message
          setMessages((prev) => [
            ...prev,
            {
              id: generateId(),
              role: 'assistant',
              content: '',
              timestamp: new Date(),
              diffImage,
            },
          ]);
        }
        break;
      }

      case 'message': {
        // Full message (non-streamed response)
        const content = (payload as { content?: string })?.content ?? String(payload);
        const diffImg = (payload as { diff_image?: string })?.diff_image;
        streamingMessageRef.current = null;
        setIsWaitingForResponse(false);
        setMessages((prev) => [
          ...prev,
          {
            id: generateId(),
            role: 'assistant',
            content,
            timestamp: new Date(),
            diffImage: diffImg,
          },
        ]);
        break;
      }

      default:
        // Unknown message type — ignore
        break;
    }
  }, []);

  const { status, send } = useWebSocket({
    token: token ?? '',
    endpoint: '/api/v1/ws/chat',
    onMessage: handleWsMessage,
  });

  // Send a message
  const sendMessage = useCallback(() => {
    const content = inputValue.trim();
    if (!content || status !== 'connected') return;

    // Add user message to the list
    const userMessage: ChatMessage = {
      id: generateId(),
      role: 'user',
      content,
      timestamp: new Date(),
    };
    setMessages((prev) => [...prev, userMessage]);
    setInputValue('');
    setIsWaitingForResponse(true);

    // Send via WebSocket
    send({
      type: 'message',
      payload: {
        session_id: SESSION_ID,
        content,
      },
    });

    // Reset textarea height
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [inputValue, status, send]);

  // Handle Enter key (send) and Shift+Enter (newline)
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
      }
    },
    [sendMessage]
  );

  // Auto-resize textarea
  const handleInputChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      setInputValue(e.target.value);
      const textarea = e.target;
      textarea.style.height = 'auto';
      textarea.style.height = `${Math.min(textarea.scrollHeight, 160)}px`;
    },
    []
  );

  const canSend = inputValue.trim().length > 0 && status === 'connected';

  return (
    <div className={styles.container}>
      {/* Header with connection status */}
      <div className={styles.header}>
        <h1 className={styles.headerTitle}>Chat</h1>
        <div className={styles.headerStatus}>
          <span
            className={`${styles.statusDot} ${getStatusDotClass(status)}`}
            aria-label={`Connection status: ${status}`}
          />
          <span>{getStatusLabel(status)}</span>
        </div>
      </div>

      {/* Connection failure banner */}
      {status === 'reconnecting' && (
        <div className={`${styles.connectionBanner} ${styles.connectionBannerReconnecting}`}>
          <span className={`${styles.statusDot} ${styles.statusDotReconnecting}`} />
          Reconnecting to server...
        </div>
      )}
      {status === 'disconnected' && (
        <div className={`${styles.connectionBanner} ${styles.connectionBannerDisconnected}`}>
          <span className={`${styles.statusDot} ${styles.statusDotDisconnected}`} />
          Connection lost. Retrying every 30 seconds...
        </div>
      )}

      {/* Message list */}
      <div className={styles.messageList} ref={messageListRef} role="log" aria-live="polite">
        {messages.length === 0 && (
          <div className={styles.emptyState}>
            <span className={styles.emptyStateTitle}>Start a conversation</span>
            <span className={styles.emptyStateText}>
              Send a message to begin chatting with XenoClaw.
            </span>
          </div>
        )}

        {messages.map((msg) => (
          <div
            key={msg.id}
            className={`${styles.message} ${getMessageClass(msg.role)}`}
          >
            {msg.content && (
              <span className={styles.messageContent}>{msg.content}</span>
            )}
            {msg.diffImage && renderDiffImage(msg.diffImage)}
            <span className={styles.messageTimestamp}>
              {formatTimestamp(msg.timestamp)}
            </span>
          </div>
        ))}

        {/* Loading indicator while waiting for response */}
        {isWaitingForResponse && !streamingMessageRef.current && (
          <div className={styles.typingIndicator} aria-label="Assistant is typing">
            <span className={styles.typingDot} />
            <span className={styles.typingDot} />
            <span className={styles.typingDot} />
          </div>
        )}
      </div>

      {/* Input area */}
      <div className={styles.inputArea}>
        <div className={styles.inputWrapper}>
          <textarea
            ref={textareaRef}
            className={styles.input}
            value={inputValue}
            onChange={handleInputChange}
            onKeyDown={handleKeyDown}
            placeholder={
              status === 'connected'
                ? 'Type a message... (Enter to send, Shift+Enter for newline)'
                : 'Waiting for connection...'
            }
            disabled={status !== 'connected'}
            rows={1}
            aria-label="Message input"
          />
        </div>
        <button
          className={styles.sendButton}
          onClick={sendMessage}
          disabled={!canSend}
          aria-label="Send message"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="22" y1="2" x2="11" y2="13" />
            <polygon points="22 2 15 22 11 13 2 9 22 2" />
          </svg>
        </button>
      </div>
    </div>
  );
}

/** Get CSS class for the status dot based on connection status */
function getStatusDotClass(status: ConnectionStatus): string {
  switch (status) {
    case 'connected':
      return styles.statusDotConnected;
    case 'reconnecting':
    case 'connecting':
      return styles.statusDotReconnecting;
    case 'disconnected':
      return styles.statusDotDisconnected;
  }
}

/** Get human-readable label for connection status */
function getStatusLabel(status: ConnectionStatus): string {
  switch (status) {
    case 'connected':
      return 'Connected';
    case 'connecting':
      return 'Connecting...';
    case 'reconnecting':
      return 'Reconnecting...';
    case 'disconnected':
      return 'Disconnected';
  }
}

/** Get CSS class for message bubble based on role */
function getMessageClass(role: ChatMessage['role']): string {
  switch (role) {
    case 'user':
      return styles.messageUser;
    case 'assistant':
      return styles.messageAssistant;
    case 'error':
      return styles.messageError;
  }
}

/** Render a diff image — either as an <img> tag (URL) or inline SVG */
function renderDiffImage(diffImage: string) {
  // Check if it's SVG data (starts with < or contains <svg)
  if (diffImage.trim().startsWith('<')) {
    return (
      <div
        className={styles.diffSvgContainer}
        dangerouslySetInnerHTML={{ __html: diffImage }}
        aria-label="Code diff visualization"
      />
    );
  }

  // Otherwise treat as image URL
  return (
    <img
      className={styles.diffImage}
      src={diffImage}
      alt="Code diff visualization"
      loading="lazy"
    />
  );
}

/** Format a timestamp for display */
function formatTimestamp(date: Date): string {
  return date.toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
  });
}
