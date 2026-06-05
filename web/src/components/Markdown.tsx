import {
  useCallback,
  useRef,
  useState,
  type ReactNode,
  isValidElement,
} from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import styles from './Markdown.module.css';

/** Pull the `language-xxx` hint rehype-highlight leaves on the <code> child. */
function langFromChild(children: ReactNode): string | null {
  if (isValidElement(children)) {
    const cls = (children.props as { className?: string }).className ?? '';
    const match = /language-([\w-]+)/.exec(cls);
    if (match) return match[1];
  }
  return null;
}

/** A fenced code block: language label + copy button over the highlighted code. */
function CodeBlock({ children }: { children?: ReactNode }) {
  const preRef = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const lang = langFromChild(children);

  const copy = useCallback(async () => {
    const text = preRef.current?.textContent ?? '';
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* clipboard blocked (e.g. insecure origin) — silently no-op */
    }
  }, []);

  return (
    <div className={styles.codeWrap}>
      <div className={styles.codeBar}>
        <span className={styles.codeLang}>{lang ?? 'text'}</span>
        <button
          type="button"
          className={styles.copyBtn}
          onClick={() => void copy()}
          aria-label="Copy code"
        >
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre ref={preRef}>{children}</pre>
    </div>
  );
}

/**
 * Renders assistant message Markdown with GFM + syntax highlighting.
 * Behaviour is presentation-only; the message text comes through unchanged.
 */
export function Markdown({ content }: { content: string }) {
  return (
    <div className={`md ${styles.body}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[[rehypeHighlight, { ignoreMissing: true, detect: true }]]}
        components={{
          pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
          // Links open in a new tab — assistant output may reference URLs.
          a: ({ children, href }) => (
            <a href={href} target="_blank" rel="noopener noreferrer">
              {children}
            </a>
          ),
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
