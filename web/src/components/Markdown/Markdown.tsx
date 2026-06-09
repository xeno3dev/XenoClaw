import { useState, type ReactNode } from 'react';
import styles from './Markdown.module.css';

/**
 * Small, dependency-free Markdown renderer tuned for chat messages.
 *
 * Supports: fenced code blocks (with a copy button + language label), inline
 * code, bold/italic/strikethrough, links, headings, ordered/unordered lists,
 * blockquotes, and horizontal rules. All text is rendered as React children, so
 * it is XSS-safe (no dangerouslySetInnerHTML). This deliberately avoids pulling
 * in react-markdown + a syntax highlighter to keep the bundle small.
 */
export function Markdown({ content }: { content: string }): ReactNode {
  const blocks = parseBlocks(content);
  return <div className={styles.md}>{blocks.map((b, i) => renderBlock(b, i))}</div>;
}

// ---- Block model ------------------------------------------------------------

type Block =
  | { type: 'code'; lang: string; text: string }
  | { type: 'heading'; level: number; text: string }
  | { type: 'ul'; items: string[] }
  | { type: 'ol'; items: string[] }
  | { type: 'quote'; text: string }
  | { type: 'hr' }
  | { type: 'p'; text: string };

const FENCE_RE = /```([^\n`]*)\n([\s\S]*?)```/g;

function parseBlocks(src: string): Block[] {
  const blocks: Block[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  FENCE_RE.lastIndex = 0;
  while ((m = FENCE_RE.exec(src)) !== null) {
    if (m.index > last) {
      blocks.push(...parseTextBlocks(src.slice(last, m.index)));
    }
    blocks.push({ type: 'code', lang: m[1].trim(), text: m[2].replace(/\n$/, '') });
    last = m.index + m[0].length;
  }
  if (last < src.length) {
    blocks.push(...parseTextBlocks(src.slice(last)));
  }
  return blocks;
}

function parseTextBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n/g, '\n').split('\n');
  const blocks: Block[] = [];
  let para: string[] = [];

  const flushPara = () => {
    if (para.length) {
      blocks.push({ type: 'p', text: para.join('\n').trim() });
      para = [];
    }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trim();

    if (trimmed === '') {
      flushPara();
      continue;
    }
    if (/^#{1,6}\s+/.test(trimmed)) {
      flushPara();
      const level = trimmed.match(/^#+/)![0].length;
      blocks.push({ type: 'heading', level, text: trimmed.replace(/^#+\s+/, '') });
      continue;
    }
    if (/^(---|\*\*\*|___)\s*$/.test(trimmed)) {
      flushPara();
      blocks.push({ type: 'hr' });
      continue;
    }
    if (/^[-*+]\s+/.test(trimmed)) {
      flushPara();
      const items: string[] = [];
      while (i < lines.length && /^\s*[-*+]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*[-*+]\s+/, ''));
        i++;
      }
      i--;
      blocks.push({ type: 'ul', items });
      continue;
    }
    if (/^\d+\.\s+/.test(trimmed)) {
      flushPara();
      const items: string[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ''));
        i++;
      }
      i--;
      blocks.push({ type: 'ol', items });
      continue;
    }
    if (/^>\s?/.test(trimmed)) {
      flushPara();
      const quote: string[] = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        quote.push(lines[i].replace(/^\s*>\s?/, ''));
        i++;
      }
      i--;
      blocks.push({ type: 'quote', text: quote.join('\n') });
      continue;
    }
    para.push(line);
  }
  flushPara();
  return blocks;
}

function renderBlock(block: Block, key: number): ReactNode {
  switch (block.type) {
    case 'code':
      return <CodeBlock key={key} lang={block.lang} text={block.text} />;
    case 'heading': {
      const children = renderInline(block.text);
      const level = Math.min(block.level + 2, 6);
      if (level <= 3) return <h3 key={key} className={styles.heading}>{children}</h3>;
      if (level === 4) return <h4 key={key} className={styles.heading}>{children}</h4>;
      if (level === 5) return <h5 key={key} className={styles.heading}>{children}</h5>;
      return <h6 key={key} className={styles.heading}>{children}</h6>;
    }
    case 'ul':
      return (
        <ul key={key} className={styles.list}>
          {block.items.map((it, i) => (
            <li key={i}>{renderInline(it)}</li>
          ))}
        </ul>
      );
    case 'ol':
      return (
        <ol key={key} className={styles.list}>
          {block.items.map((it, i) => (
            <li key={i}>{renderInline(it)}</li>
          ))}
        </ol>
      );
    case 'quote':
      return (
        <blockquote key={key} className={styles.quote}>
          {renderInline(block.text)}
        </blockquote>
      );
    case 'hr':
      return <hr key={key} className={styles.hr} />;
    case 'p':
      return (
        <p key={key} className={styles.p}>
          {renderInline(block.text)}
        </p>
      );
  }
}

// ---- Inline rendering -------------------------------------------------------

interface InlineRule {
  re: RegExp;
  render: (m: RegExpExecArray, key: string, recurse: (s: string) => ReactNode[]) => ReactNode;
}

const INLINE_RULES: InlineRule[] = [
  { re: /`([^`]+)`/, render: (m, key) => <code key={key} className={styles.inlineCode}>{m[1]}</code> },
  {
    re: /\[([^\]]+)\]\(([^)\s]+)\)/,
    render: (m, key, recurse) => (
      <a key={key} className={styles.link} href={m[2]} target="_blank" rel="noreferrer noopener">
        {recurse(m[1])}
      </a>
    ),
  },
  { re: /\*\*([^*]+)\*\*/, render: (m, key, recurse) => <strong key={key}>{recurse(m[1])}</strong> },
  { re: /__([^_]+)__/, render: (m, key, recurse) => <strong key={key}>{recurse(m[1])}</strong> },
  { re: /~~([^~]+)~~/, render: (m, key, recurse) => <del key={key}>{recurse(m[1])}</del> },
  { re: /\*([^*]+)\*/, render: (m, key, recurse) => <em key={key}>{recurse(m[1])}</em> },
  { re: /(?<![A-Za-z0-9])_([^_]+)_(?![A-Za-z0-9])/, render: (m, key, recurse) => <em key={key}>{recurse(m[1])}</em> },
];

let keyCounter = 0;

function renderInline(text: string): ReactNode[] {
  let best: { rule: InlineRule; m: RegExpExecArray } | null = null;
  for (const rule of INLINE_RULES) {
    const m = rule.re.exec(text);
    if (m && (best === null || m.index < best.m.index)) {
      best = { rule, m };
    }
  }
  if (!best) {
    return [renderText(text)];
  }
  const { m, rule } = best;
  const before = text.slice(0, m.index);
  const after = text.slice(m.index + m[0].length);
  const nodes: ReactNode[] = [];
  if (before) nodes.push(renderText(before));
  nodes.push(rule.render(m, `i${keyCounter++}`, renderInline));
  if (after) nodes.push(...renderInline(after));
  return nodes;
}

/** Render plain text, converting single newlines to <br>. */
function renderText(text: string): ReactNode {
  const parts = text.split('\n');
  if (parts.length === 1) return text;
  return parts.map((part, i) => (
    <span key={`t${keyCounter++}`}>
      {part}
      {i < parts.length - 1 && <br />}
    </span>
  ));
}

// ---- Code block with copy button -------------------------------------------

function CodeBlock({ lang, text }: { lang: string; text: string }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };

  return (
    <div className={styles.codeBlock}>
      <div className={styles.codeHeader}>
        <span className={styles.codeLang}>{lang || 'text'}</span>
        <button className={styles.copyButton} onClick={() => void copy()} type="button">
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre className={styles.pre}>
        <code>{text}</code>
      </pre>
    </div>
  );
}
