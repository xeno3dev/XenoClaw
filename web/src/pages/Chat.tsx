import { useState, useCallback, useRef, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import { useWebSocket, type ConnectionStatus, type WebSocketMessage } from '../hooks/useWebSocket';
import styles from './Chat.module.css';

/** Agent operating modes, mirroring Claude Code / OpenCode. */
type AgentMode = 'general' | 'plan' | 'code';

const AGENT_MODES: { id: AgentMode; label: string }[] = [
  { id: 'general', label: 'General' },
  { id: 'plan', label: 'Plan' },
  { id: 'code', label: 'Code' },
];

function isAgentMode(v: unknown): v is AgentMode {
  return v === 'general' || v === 'plan' || v === 'code';
}

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
  /** Filenames attached to this (user) message */
  attachments?: string[];
}

/** A file the user has uploaded for the current message. */
interface PendingAttachment {
  /** Display name */
  name: string;
  /** Workspace-relative path returned by the upload endpoint */
  path: string;
  isImage: boolean;
}

/** Generate a session UUID. The backend validates session_id as a UUID. */
function newSessionId(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  // Secure fallback using getRandomValues
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  return [...bytes].map((b, i) =>
    [4, 6, 8, 10].includes(i) ? '-' + b.toString(16).padStart(2, '0') : b.toString(16).padStart(2, '0')
  ).join('');
}

/**
 * Chat page — real-time chat interface with WebSocket streaming.
 * Implements Requirements 7.1, 7.4, 7.5, 16.4
 */
export function Chat() {
  const { token, apiFetch } = useAuth();
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [inputValue, setInputValue] = useState('');
  const [isWaitingForResponse, setIsWaitingForResponse] = useState(false);
  const [agentMode, setAgentMode] = useState<AgentMode>('general');
  const [attachments, setAttachments] = useState<PendingAttachment[]>([]);
  const [uploading, setUploading] = useState(false);

  const messageListRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const streamingMessageRef = useRef<string | null>(null);
  // Stable per-mount session id (UUID — the backend requires it).
  const sessionIdRef = useRef<string>(newSessionId());

  // Fetch the current agent mode from the status endpoint on mount
  useEffect(() => {
    apiFetch('/api/v1/status')
      .then((res) => (res.ok ? (res.json() as Promise<{ mode?: string }>) : null))
      .then((data) => {
        if (isAgentMode(data?.mode)) {
          setAgentMode(data.mode);
        }
      })
      .catch(() => {}); // network errors are non-fatal — keep default
  }, [apiFetch]);

  // Switch the agent mode and persist via the config endpoint
  const setMode = useCallback(
    async (newMode: AgentMode) => {
      if (newMode === agentMode) return;
      setAgentMode(newMode);
      try {
        const res = await apiFetch('/api/v1/config', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ settings: { mode: newMode } }),
        });
        if (!res.ok) setAgentMode(agentMode); // revert on error
      } catch {
        setAgentMode(agentMode); // revert on network failure
      }
    },
    [agentMode, apiFetch],
  );

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

  // Upload selected files to the session's upload directory, then track them
  // as pending attachments for the next message.
  const handleFilesSelected = useCallback(
    async (fileList: FileList | null) => {
      if (!fileList || fileList.length === 0) return;
      setUploading(true);
      try {
        const form = new FormData();
        Array.from(fileList).forEach((f) => form.append('files', f, f.name));
        const res = await apiFetch(`/api/v1/uploads/${sessionIdRef.current}`, {
          method: 'POST',
          body: form,
        });
        if (!res.ok) throw new Error(`Upload failed (${res.status})`);
        const data = (await res.json()) as {
          files: { name: string; path: string; is_image: boolean }[];
        };
        setAttachments((prev) => [
          ...prev,
          ...data.files.map((f) => ({ name: f.name, path: f.path, isImage: f.is_image })),
        ]);
      } catch (err) {
        setMessages((prev) => [
          ...prev,
          {
            id: generateId(),
            role: 'error',
            content: err instanceof Error ? err.message : 'Failed to upload file(s)',
            timestamp: new Date(),
          },
        ]);
      } finally {
        setUploading(false);
        if (fileInputRef.current) fileInputRef.current.value = '';
      }
    },
    [apiFetch],
  );

  const removeAttachment = useCallback((path: string) => {
    setAttachments((prev) => prev.filter((a) => a.path !== path));
  }, []);

  // Send a message
  const sendMessage = useCallback(() => {
    const content = inputValue.trim();
    // Allow sending if there's text OR attachments.
    if ((!content && attachments.length === 0) || status !== 'connected') return;

    const attachmentPaths = attachments.map((a) => a.path);

    // Add user message to the list
    const userMessage: ChatMessage = {
      id: generateId(),
      role: 'user',
      content,
      timestamp: new Date(),
      attachments: attachmentPaths.length > 0 ? attachmentPaths : undefined,
    };
    setMessages((prev) => [...prev, userMessage]);
    setInputValue('');
    setAttachments([]);
    setIsWaitingForResponse(true);

    // Send via WebSocket.
    send({
      type: 'message',
      payload: {
        session_id: sessionIdRef.current,
        content,
        attachments: attachmentPaths,
      },
    });

    // Reset textarea height
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
    }
  }, [inputValue, attachments, status, send]);

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

  const canSend =
    (inputValue.trim().length > 0 || attachments.length > 0) && status === 'connected';

  return (
    <div className={styles.container}>
      {/* Header with mode toggle and connection status */}
      <div className={styles.header}>
        <h1 className={styles.headerTitle}>Chat</h1>
        <div className={styles.headerRight}>
          <div className={styles.modeToggle} role="group" aria-label="Agent mode">
            {AGENT_MODES.map((m) => (
              <button
                key={m.id}
                className={`${styles.modeBtn} ${agentMode === m.id ? styles.modeBtnActive : ''}`}
                onClick={() => void setMode(m.id)}
                aria-pressed={agentMode === m.id}
                title={
                  m.id === 'plan'
                    ? 'Plan mode: read-only — investigates and proposes, no writes'
                    : m.id === 'code'
                      ? 'Code mode: full dev tools including file writes and shell'
                      : 'General mode: base tools only'
                }
              >
                {m.label}
              </button>
            ))}
          </div>
          <div className={styles.headerStatus}>
            <span
              className={`${styles.statusDot} ${getStatusDotClass(status)}`}
              aria-label={`Connection status: ${status}`}
            />
            <span>{getStatusLabel(status)}</span>
          </div>
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
            {msg.attachments && msg.attachments.length > 0 && (
              <div className={styles.messageAttachments}>
                {msg.attachments.map((path) => (
                  <span key={path} className={styles.messageAttachmentChip}>
                    {path.split('/').pop()}
                  </span>
                ))}
              </div>
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

      {/* Pending attachment chips */}
      {attachments.length > 0 && (
        <div className={styles.attachmentBar}>
          {attachments.map((a) => (
            <span key={a.path} className={styles.attachmentChip}>
              <span className={styles.attachmentIcon} aria-hidden="true">
                {a.isImage ? '🖼' : '📄'}
              </span>
              <span className={styles.attachmentName}>{a.name}</span>
              <button
                className={styles.attachmentRemove}
                onClick={() => removeAttachment(a.path)}
                aria-label={`Remove ${a.name}`}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}

      {/* Input area */}
      <div className={styles.inputArea}>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          className={styles.hiddenFileInput}
          onChange={(e) => void handleFilesSelected(e.target.files)}
          aria-hidden="true"
          tabIndex={-1}
        />
        <button
          className={styles.attachButton}
          onClick={() => fileInputRef.current?.click()}
          disabled={status !== 'connected' || uploading}
          aria-label="Attach files"
          title="Attach files or images"
        >
          {uploading ? (
            <span className={styles.attachSpinner} aria-hidden="true" />
          ) : (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
            </svg>
          )}
        </button>
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
