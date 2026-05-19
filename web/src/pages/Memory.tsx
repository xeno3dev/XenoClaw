import { useState, useCallback } from 'react';
import type { FormEvent } from 'react';
import { useAuth } from '../hooks/useAuth';
import styles from './Memory.module.css';

const API_BASE = '/api/v1';

interface MemoryEntry {
  id: string;
  title: string;
  content: string;
  tags: string[];
  relevance?: number;
}

export function Memory() {
  const { token } = useAuth();
  const [entries, setEntries] = useState<MemoryEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasSearched, setHasSearched] = useState(false);
  const [showForm, setShowForm] = useState(false);

  // Form state
  const [formTitle, setFormTitle] = useState('');
  const [formContent, setFormContent] = useState('');
  const [formTags, setFormTags] = useState('');

  const headers = useCallback(() => ({
    'Authorization': `Bearer ${token}`,
    'Content-Type': 'application/json',
  }), [token]);

  const searchMemory = useCallback(async (e: FormEvent) => {
    e.preventDefault();
    if (!searchQuery.trim()) return;

    setLoading(true);
    setHasSearched(true);
    try {
      const res = await fetch(
        `${API_BASE}/memory/search?q=${encodeURIComponent(searchQuery.trim())}`,
        { headers: headers() }
      );
      if (!res.ok) throw new Error(`Search failed (${res.status})`);
      const data = await res.json() as { results: MemoryEntry[] };
      setEntries(data.results ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Search failed');
    } finally {
      setLoading(false);
    }
  }, [headers, searchQuery]);

  const storeEntry = useCallback(async (e: FormEvent) => {
    e.preventDefault();
    if (!formTitle.trim() || !formContent.trim()) return;

    try {
      const tags = formTags
        .split(',')
        .map((t) => t.trim())
        .filter((t) => t.length > 0);

      const res = await fetch(`${API_BASE}/memory`, {
        method: 'POST',
        headers: headers(),
        body: JSON.stringify({
          title: formTitle.trim(),
          content: formContent.trim(),
          tags,
        }),
      });
      if (!res.ok) throw new Error(`Failed to store entry (${res.status})`);
      setFormTitle('');
      setFormContent('');
      setFormTags('');
      setShowForm(false);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to store entry');
    }
  }, [headers, formTitle, formContent, formTags]);

  const deleteEntry = useCallback(async (id: string) => {
    try {
      const res = await fetch(`${API_BASE}/memory/${id}`, {
        method: 'DELETE',
        headers: headers(),
      });
      if (!res.ok) throw new Error(`Failed to delete entry (${res.status})`);
      setEntries((prev) => prev.filter((e) => e.id !== id));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete entry');
    }
  }, [headers]);

  return (
    <div className={styles.container}>
      <div className={styles.header}>
        <h1 className={styles.title}>Memory</h1>
        <button
          className={styles.createButton}
          onClick={() => setShowForm((prev) => !prev)}
        >
          {showForm ? 'Cancel' : '+ Store Entry'}
        </button>
      </div>

      {error && (
        <div className={styles.errorBanner} role="alert">
          <span className={styles.errorIcon}>⚠</span>
          {error}
        </div>
      )}

      {/* Store Entry Form */}
      {showForm && (
        <form className={styles.form} onSubmit={storeEntry}>
          <div className={styles.field}>
            <label className={styles.fieldLabel} htmlFor="memory-title">Title</label>
            <input
              id="memory-title"
              className={styles.input}
              type="text"
              value={formTitle}
              onChange={(e) => setFormTitle(e.target.value)}
              placeholder="Entry title"
              required
            />
          </div>
          <div className={styles.field}>
            <label className={styles.fieldLabel} htmlFor="memory-content">Content</label>
            <textarea
              id="memory-content"
              className={styles.textarea}
              value={formContent}
              onChange={(e) => setFormContent(e.target.value)}
              placeholder="Knowledge content..."
              rows={4}
              required
            />
          </div>
          <div className={styles.field}>
            <label className={styles.fieldLabel} htmlFor="memory-tags">Tags (comma-separated)</label>
            <input
              id="memory-tags"
              className={styles.input}
              type="text"
              value={formTags}
              onChange={(e) => setFormTags(e.target.value)}
              placeholder="tag1, tag2, tag3"
            />
          </div>
          <button className={styles.submitButton} type="submit">
            Store Entry
          </button>
        </form>
      )}

      {/* Search */}
      <form className={styles.searchForm} onSubmit={searchMemory}>
        <input
          className={styles.searchInput}
          type="text"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          placeholder="Search knowledge base..."
        />
        <button className={styles.searchButton} type="submit" disabled={loading}>
          {loading ? 'Searching...' : 'Search'}
        </button>
      </form>

      {/* Results */}
      {hasSearched && entries.length === 0 && !loading && (
        <p className={styles.emptyState}>No results found</p>
      )}

      {entries.length > 0 && (
        <div className={styles.list}>
          {entries.map((entry) => (
            <div key={entry.id} className={styles.entryCard}>
              <div className={styles.entryHeader}>
                <span className={styles.entryTitle}>{entry.title}</span>
                {entry.relevance !== undefined && (
                  <span className={styles.relevance}>
                    {(entry.relevance * 100).toFixed(0)}% match
                  </span>
                )}
              </div>
              <p className={styles.entryContent}>
                {entry.content.length > 200
                  ? `${entry.content.slice(0, 200)}...`
                  : entry.content}
              </p>
              {entry.tags.length > 0 && (
                <div className={styles.tags}>
                  {entry.tags.map((tag) => (
                    <span key={tag} className={styles.tag}>{tag}</span>
                  ))}
                </div>
              )}
              <button
                className={styles.deleteButton}
                onClick={() => deleteEntry(entry.id)}
                title="Delete entry"
              >
                ✕
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
