# Implementation Plan: VPS AI Agent Platform

## Overview

This plan implements the VPS AI Agent Platform as a modular Rust workspace with a React/Vite web interface. Tasks are ordered to build foundational infrastructure first (config, data models, security), then core runtime (agent core, LLM router, scheduler), followed by optional modules (coding, messaging), interfaces (API, web, TUI), and finally deployment and observability. Each task builds incrementally on previous work.

## Tasks

- [x] 1. Project scaffolding and core data models
  - [x] 1.1 Initialize Rust workspace and project structure
    - Create Cargo workspace with crates: `agent-core`, `llm-router`, `task-scheduler`, `security-layer`, `coding-module`, `plugin-system`, `memory-store`, `messaging-integration`, `api-server`, `tui`, `process-supervisor`, `common`
    - Set up shared dependencies in workspace Cargo.toml (tokio, serde, uuid, chrono, tracing, thiserror, axum, sqlx)
    - Create `common` crate with shared types: SessionId, TaskId, MessageId, UserId, KnowledgeId, ApiKeyId
    - Initialize React/Vite project under `web/` for the Web Interface
    - _Requirements: 12.1, 12.2_

  - [x] 1.2 Define core data models and error types
    - Implement Message, MessageRole, ToolCall, ToolResult structs in `common`
    - Implement Task, TaskRun, TaskStatus, TaskRunStatus, TaskAction, TaskTrigger models
    - Implement User, ApiKey, Session, MessagingIdentity, Role models
    - Implement PlatformError enum hierarchy (LlmError, SecurityError, StoreError, TaskError, PluginError, ConfigError)
    - _Requirements: 1.1–1.7, 2.1–2.8, 9.4_

  - [x] 1.3 Implement configuration system (Config_Manager)
    - Implement PlatformConfig, LlmConfig, ProviderConfig, SecurityConfig, CodingConfig, MonitoringConfig structs with serde deserialization
    - Implement TOML file loading and environment variable override (env takes precedence)
    - Implement configuration validation that reports all invalid settings by name with reason
    - Refuse to start if any configuration errors exist
    - _Requirements: 12.3, 12.4, 12.6_

  - [x] 1.4 Write property tests for configuration system
    - **Property 22: Configuration Precedence (Env over TOML)**
    - **Property 23: Configuration Validation Completeness**
    - **Validates: Requirements 12.3, 12.4**

- [x] 2. Checkpoint - Verify project structure and configuration
  - Ensure all crates compile, configuration loading works with TOML + env override, and validation rejects invalid configs. Ask the user if questions arise.

- [x] 3. Security Layer implementation
  - [x] 3.1 Implement authentication (API keys and username/password)
    - Implement API key validation (minimum 32 characters)
    - Implement bcrypt password hashing and verification (minimum 12 character passwords)
    - Implement session token generation and validation with 30-minute inactivity timeout
    - Implement uniform error responses that don't reveal resource existence
    - _Requirements: 10.1, 10.2, 10.3, 10.7_

  - [x] 3.2 Write property tests for authentication
    - **Property 14: API Key Length Validation**
    - **Property 17: Authentication Response Uniformity**
    - **Property 18: Password Length Validation**
    - **Property 21: Session Timeout Enforcement**
    - **Validates: Requirements 9.3, 10.1, 10.2, 10.3, 10.7**

  - [x] 3.3 Implement brute-force protection and rate limiting
    - Track failed authentication attempts per IP address
    - Block IP after 5 consecutive failures within 10-minute window for 15 minutes
    - Implement per-API-key rate limiting (default 100 req/min)
    - Return 429 with Retry-After header when rate limit exceeded
    - _Requirements: 10.4, 10.5, 9.6, 9.7_

  - [x] 3.4 Write property tests for brute-force and rate limiting
    - **Property 16: Rate Limit Enforcement**
    - **Property 19: Brute-Force Lockout Enforcement**
    - **Validates: Requirements 9.6, 9.7, 10.4**

  - [x] 3.5 Implement RBAC authorization engine
    - Define admin and operator roles with permission sets
    - Admin: manage users, roles, all system configuration
    - Operator: execute tasks, view logs, cannot modify users or security settings
    - Implement authorize() check against role permission sets
    - _Requirements: 10.6_

  - [x] 3.6 Write property tests for RBAC
    - **Property 20: RBAC Authorization Correctness**
    - **Validates: Requirements 10.6**

  - [x] 3.7 Implement sandbox enforcement (filesystem, network, resources)
    - Implement filesystem path validation with canonical resolution (handles `..`, symlinks, relative paths)
    - Enforce per-directory read/write/execute permissions
    - Implement network access control with default-deny policy and allowlist
    - Implement CPU and memory limits for spawned processes (terminate within 5 seconds on violation)
    - _Requirements: 11.1, 11.2, 11.3, 11.4, 11.5, 11.6, 11.7_

  - [x] 3.8 Write property tests for sandbox enforcement
    - **Property 8: Filesystem Path Sandbox Validation**
    - **Property 11: Shell Command Allowlist/Blocklist Enforcement**
    - **Property 13: Network Access Control Default-Deny**
    - **Validates: Requirements 4.3, 5.4, 5.5, 11.1, 11.3, 11.6, 11.7**

  - [x] 3.9 Implement audit logging
    - Log all authentication attempts (timestamp, source IP, username/key ID, outcome)
    - Log security violations (sandbox breaches, blocked IPs, denied commands)
    - Store in audit_log table
    - _Requirements: 10.5, 11.4_

- [x] 4. Checkpoint - Security layer complete
  - Ensure all security tests pass, authentication/authorization/sandbox work correctly. Ask the user if questions arise.

- [x] 5. Memory Store implementation
  - [x] 5.1 Set up SQLite database with schema and migrations
    - Create SQLite database initialization with all tables (sessions, messages, knowledge, tasks, task_runs, users, api_keys, messaging_identities, audit_log, file_changes)
    - Set up sqlite-vec extension for vector embeddings
    - Create all indexes for performance
    - _Requirements: 14.1, 14.2_

  - [x] 5.2 Implement conversation history persistence
    - Implement store_message() and get_history() with session scoping
    - Support at least 1000 messages per session retention
    - Ensure messages include content, role, timestamps, tool calls/results, token count
    - Implement session isolation (messages in one session never appear in another)
    - _Requirements: 14.1, 14.2_

  - [x] 5.3 Write property tests for conversation persistence
    - **Property 26: Conversation History Persistence Round-Trip**
    - **Property 27: Session Context Isolation**
    - **Validates: Requirements 14.1, 14.2**

  - [x] 5.4 Implement semantic search and knowledge storage
    - Implement embedding generation for messages and knowledge entries
    - Implement semantic search over stored content using sqlite-vec (ranked results within 2 seconds for up to 100,000 entries)
    - Implement explicit knowledge storage (up to 10,000 characters per entry)
    - Implement knowledge deletion (permanent, no longer retrievable)
    - Implement capacity enforcement (reject new entries when full, never delete existing)
    - _Requirements: 14.3, 14.5, 14.6, 14.8_

  - [x] 5.5 Write property tests for knowledge and capacity
    - **Property 29: Knowledge Entry Size Validation**
    - **Property 30: Memory Store Capacity Enforcement**
    - **Validates: Requirements 14.5, 14.8**

  - [x] 5.6 Implement session state persistence and recovery
    - Implement full session state serialization (task queue, conversation history, scheduler config)
    - Implement state restoration on restart
    - Implement fallback to in-memory context when store unavailable (retry every 30 seconds)
    - _Requirements: 2.3, 14.7_

  - [x] 5.7 Write property tests for session state round-trip
    - **Property 3: Session State Persistence Round-Trip**
    - **Validates: Requirements 2.3**

- [x] 6. Checkpoint - Memory store complete
  - Ensure persistence, search, and recovery all work. Ask the user if questions arise.

- [x] 7. LLM Router implementation
  - [x] 7.1 Implement LLM provider clients
    - Implement OpenAI-compatible API client (async HTTP with configurable timeout 5–120s, default 30s)
    - Implement Anthropic Claude API client
    - Implement Ollama local inference client
    - Support streaming and non-streaming completions
    - _Requirements: 1.1, 1.2, 1.3_

  - [x] 7.2 Implement failover routing logic
    - Route requests in strict priority order
    - On timeout/failure, route to next provider (never retry same provider per request)
    - Return error with "no providers available" if all fail
    - Log all failover events (original provider, failure reason, fallback target)
    - Support 1–10 providers with per-provider config (API key, model, priority, timeout)
    - _Requirements: 1.4, 1.5, 1.6, 1.7_

  - [x] 7.3 Write property tests for LLM Router
    - **Property 1: LLM Router Failover Correctness**
    - **Property 2: LLM Provider Configuration Validation**
    - **Validates: Requirements 1.4, 1.5, 1.6**

- [x] 8. Task Scheduler implementation
  - [x] 8.1 Implement cron-based scheduling and event triggers
    - Parse and evaluate cron expressions (minimum 1-minute interval)
    - Implement file system change triggers (creation, modification, deletion of watched paths)
    - Implement webhook receipt triggers
    - Implement time-based conditional expressions
    - Execute tasks at configured times with max 60-second drift
    - _Requirements: 3.1, 3.2, 2.5_

  - [x] 8.2 Implement task dependencies and cycle detection
    - Support task dependencies (successful completion triggers dependent task)
    - Maximum dependency chain depth of 10
    - Detect cycles during task definition and reject with identification of cycle members
    - Execute tasks respecting dependency ordering (no task before its dependencies complete)
    - _Requirements: 3.5, 3.6_

  - [x] 8.3 Write property tests for task dependencies
    - **Property 6: Task Dependency Cycle Detection**
    - **Property 7: Task Dependency Execution Order**
    - **Validates: Requirements 3.5, 3.6**

  - [x] 8.4 Implement retry logic and failure handling
    - Retry failed tasks up to 3 times with exponential backoff (10s base, capped at 300s)
    - Terminate tasks exceeding configured timeout (default 300s)
    - Mark tasks as failed after retry exhaustion, log summary, continue with subsequent tasks
    - Record task history (start time, end time, duration, status, error details, retain 30 days)
    - _Requirements: 2.6, 2.8, 3.4, 3.7, 3.8_

  - [x] 8.5 Write property tests for retry and failure isolation
    - **Property 4: Exponential Backoff Retry Intervals**
    - **Property 5: Task Failure Isolation**
    - **Validates: Requirements 2.6, 2.8**

- [x] 9. Checkpoint - LLM Router and Task Scheduler complete
  - Ensure failover routing, scheduling, dependencies, and retry logic all work. Ask the user if questions arise.

- [x] 10. Agent Core and Session Manager
  - [x] 10.1 Implement Agent Core runtime
    - Implement agent lifecycle management (startup, shutdown, graceful restart)
    - Process incoming messages and route to LLM Router
    - Coordinate tool execution via Tool Registry
    - Support General and Coding agent modes
    - Target idle resource usage: ≤512MB RAM, ≤5% single CPU core
    - _Requirements: 2.1, 2.4, 3.3_

  - [x] 10.2 Implement Session Manager with context window management
    - Support at least 50 concurrent sessions with independent context
    - Summarize older context when token usage exceeds 80% of LLM token limit
    - Preserve last 10 conversation turns in full and all stored knowledge after summarization
    - _Requirements: 14.2, 14.4_

  - [x] 10.3 Write property tests for context summarization
    - **Property 28: Context Summarization Preserves Critical Content**
    - **Validates: Requirements 14.4**

  - [x] 10.4 Implement Tool Registry and Event Bus
    - Implement tool registration and lookup for Agent Core
    - Implement async channel-based event bus for component communication
    - Support plugin tool registration through the registry
    - _Requirements: 13.3, 13.4_

- [x] 11. Process Supervisor
  - [x] 11.1 Implement process supervision and auto-restart
    - Detect unexpected agent termination and restart within 10 seconds
    - Auto-start agent within 30 seconds of OS reaching default run level
    - Restore previous session state on restart (task queue, history, scheduler config)
    - Restart agent if unresponsive for more than 60 seconds
    - Maintain uptime and restart history statistics
    - _Requirements: 2.2, 2.7, 18.4_

- [x] 12. Coding Module — File Operations
  - [x] 12.1 Implement file read, create, edit, and delete operations
    - Implement file read/create/write/delete as tools registered with Tool Registry
    - Implement precise string replacement (reject if match string not found)
    - Enforce 10MB file size limit (reject operations on larger files)
    - Validate all paths through Security Layer sandbox (reject paths outside workspace, including symlinks and relative components)
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.6, 4.7_

  - [x] 12.2 Write property tests for file operations
    - **Property 9: File Edit String Replacement Correctness**
    - **Validates: Requirements 4.2, 4.6**

  - [x] 12.3 Implement undo history for file operations
    - Track last 50 file-level operations (create, edit, delete each count as one)
    - Implement rollback that restores filesystem to state before operations
    - _Requirements: 4.5_

  - [x] 12.4 Write property tests for undo history
    - **Property 10: File Operation Undo Round-Trip**
    - **Validates: Requirements 4.5**

- [x] 13. Coding Module — Terminal Access
  - [x] 13.1 Implement shell command execution
    - Execute shell commands capturing stdout, stderr, and exit code
    - Stream output with ≤2 seconds latency
    - Enforce configurable timeout (default 300s), terminate on exceed
    - Support up to 5 concurrent shell processes (reject when limit reached)
    - Enforce command allowlist/blocklist (default deny-all when no allowlist)
    - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5, 5.6, 5.7, 5.8_

- [x] 14. Coding Module — Git Integration
  - [x] 14.1 Implement git operations
    - Implement clone, pull, push, commit, branch, merge, and diff operations
    - Generate commit messages: summary ≤72 chars with type and component, body listing modified files
    - Report conflicts on push failure (branch name, affected files) instead of force-pushing
    - Support SSH key and personal access token authentication
    - Restrict operations to configured repository directories
    - Report auth failures without retrying
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 6.6, 6.7_

  - [x] 14.2 Write property tests for git commit messages
    - **Property 12: Git Commit Message Format**
    - **Validates: Requirements 6.2**

- [x] 15. Coding Module — Diff Rendering and Change Tracking
  - [x] 15.1 Implement change tracking and unified diff generation
    - Track all file modifications during a session (before/after states)
    - Generate unified diffs with at least 3 context lines
    - Track each modification independently; display cumulative diff relative to original state
    - Generate session change summary (all modified files, total lines added/removed)
    - Handle binary/non-text files (indicate changed without line-level diff)
    - _Requirements: 16.1, 16.2, 16.5, 16.7, 16.8_

  - [x] 15.2 Write property tests for diff generation
    - **Property 31: Unified Diff Generation Correctness**
    - **Property 32: Cumulative Diff Equivalence**
    - **Property 33: Session Change Summary Accuracy**
    - **Validates: Requirements 16.2, 16.5, 16.7**

  - [x] 15.3 Implement diff image rendering
    - Render diffs as images with file name, line numbers, green highlights for additions, red for removals
    - Support combined diff images for multi-file operations
    - _Requirements: 16.3, 16.6_

- [x] 16. Coding Module — LSP Integration
  - [x] 16.1 Implement LSP client management
    - Initialize and maintain connections to configured language servers per workspace language
    - Request diagnostics within 2 seconds of file modification
    - Report errors to operator and withhold changes until acknowledged or resolved
    - Use go-to-definition, find-references, rename-symbol for refactoring operations
    - Handle language server failures gracefully (log, notify, continue without LSP)
    - Proceed without LSP when no server configured for a language
    - _Requirements: 15.1, 15.2, 15.3, 15.4, 15.5, 15.6, 15.7_

- [x] 17. Checkpoint - Coding Module complete
  - Ensure file operations, terminal, git, diff rendering, and LSP integration all work. Ask the user if questions arise.

- [x] 18. Plugin System
  - [x] 18.1 Implement plugin discovery and loading
    - Discover plugins from configurable directory at startup (valid manifest: name, version, API version)
    - Detect file changes and hot-reload plugins within 10 seconds (allow in-flight ops to complete/timeout within 30s)
    - Provide PluginApi for registering tools and event handlers
    - Handle load failures gracefully (log error, continue loading remaining plugins)
    - Enforce Security Layer resource limits on plugin-spawned operations
    - _Requirements: 13.1, 13.2, 13.3, 13.4, 13.5, 13.6_

  - [x] 18.2 Implement plugin conflict resolution
    - Reject duplicate tool/handler name registrations (first registration wins)
    - Log conflict error identifying both plugins
    - Continue operating with original registration intact
    - _Requirements: 13.7_

  - [x] 18.3 Write property tests for plugin system
    - **Property 24: Plugin Fault Isolation**
    - **Property 25: Plugin Name Conflict Resolution**
    - **Validates: Requirements 13.5, 13.7**

- [x] 19. API Server
  - [x] 19.1 Implement REST API endpoints
    - Implement all REST endpoints: messages, status, tasks, health, config, memory, plugins
    - Require API key authentication (Bearer token, min 32 chars) on all endpoints
    - Return structured JSON error responses (error code, message, request ID)
    - Return 400 for validation errors with description
    - Integrate rate limiting (429 + Retry-After header)
    - Implement health check endpoint (responds within 2s, returns version + healthy/unhealthy)
    - _Requirements: 9.1, 9.3, 9.4, 9.5, 9.6, 9.7, 12.5_

  - [x] 19.2 Write property tests for API error responses
    - **Property 15: API Error Response Structure**
    - **Validates: Requirements 9.4**

  - [x] 19.3 Implement WebSocket endpoints
    - Implement real-time chat streaming via WebSocket
    - Implement system event stream via WebSocket
    - Handle connection drops and reconnection
    - _Requirements: 9.2_

- [x] 20. Checkpoint - API Server complete
  - Ensure REST endpoints, WebSocket streaming, auth, rate limiting, and error responses all work. Ask the user if questions arise.

- [x] 21. Web Interface (React/Vite)
  - [x] 21.1 Set up React/Vite project with core layout
    - Initialize Vite + React + TypeScript project
    - Set up responsive layout (320px–2560px, breakpoint at 768px)
    - Configure WebSocket client for real-time communication
    - Set up HTTPS/TLS 1.2+ support
    - _Requirements: 7.6, 7.7_

  - [x] 21.2 Implement chat interface
    - Build chat UI with message display (within 2s of submission/generation)
    - Implement WebSocket-based real-time message streaming
    - Display diff images inline when coding module generates them
    - Handle WebSocket disconnection (auto-reconnect within 10s, persistent indicator if fails, retry every 30s)
    - _Requirements: 7.1, 7.4, 7.5, 16.4_

  - [x] 21.3 Implement dashboard and monitoring views
    - Display agent status (current task, resource usage, uptime) refreshed every ≤5 seconds
    - Show scheduled tasks, 50 most recent activity entries
    - Show system health indicators (CPU %, memory %, service availability)
    - _Requirements: 7.2, 7.3_

- [x] 22. Terminal UI (Ratatui)
  - [x] 22.1 Implement TUI chat and status display
    - Build scrollable message history (at least 200 recent messages) and text input area
    - Display agent status (idle/working/error), task progress, resource usage (CPU %, memory MB) refreshed every ≤2 seconds
    - Implement keyboard shortcuts (task cancellation, history navigation, mode switching, help shortcut)
    - Handle terminal resize within 100ms
    - Support 80x24 minimum terminal dimensions
    - Handle SSH latency up to 500ms without display corruption or dropped input
    - Display disconnection notification and auto-reconnect (every ≤5s, up to 6 attempts)
    - _Requirements: 8.1, 8.2, 8.3, 8.4, 8.5, 8.6, 8.7_

- [x] 23. Messaging Integration
  - [x] 23.1 Implement Telegram bot integration
    - Implement Telegram bot using teloxide
    - Process incoming messages as conversation input, respond within 30 seconds
    - Support management commands (task scheduling, status, mode switching, config)
    - Send diff images inline with responses
    - _Requirements: 17.1, 17.4, 17.5, 17.6_

  - [x] 23.2 Implement Discord bot integration
    - Implement Discord bot using serenity
    - Process messages, respond within 30 seconds
    - Support management commands and diff image delivery
    - _Requirements: 17.2, 17.4, 17.5, 17.6_

  - [x] 23.3 Implement WhatsApp bot integration
    - Implement WhatsApp bot using whatsapp-web-rs
    - Process messages, respond within 30 seconds
    - Support management commands and diff image delivery (as attachment if inline not supported)
    - _Requirements: 17.3, 17.4, 17.5, 17.6_

  - [x] 23.4 Implement messaging identity mapping and authorization
    - Map platform user IDs to operator accounts
    - Enforce same RBAC rules as Web Interface
    - Reject messages from unmapped/unauthorized users with error response
    - Validate credentials on configuration save (test connection)
    - Retry delivery up to 3 times (5s interval) when platform unreachable, log and discard on failure
    - _Requirements: 17.7, 17.8, 17.9, 17.10_

  - [x] 23.5 Write property tests for messaging authorization
    - **Property 34: Messaging Identity Authorization**
    - **Validates: Requirements 17.8, 17.9**

- [x] 24. Checkpoint - All interfaces and integrations complete
  - Ensure Web UI, TUI, and messaging integrations all communicate with the agent correctly. Ask the user if questions arise.

- [x] 25. Monitoring and Observability
  - [x] 25.1 Implement structured logging
    - Emit JSON structured logs with configurable levels (DEBUG, INFO, WARN, ERROR, FATAL)
    - Each entry includes: timestamp (ISO 8601), correlation_id (UUID), level, component, message
    - Implement log rotation (when file exceeds configured max size)
    - Implement retention (default 30 days, configurable 1–365 days, auto-cleanup)
    - Implement ring buffer (1000 entries) when logging subsystem unavailable, forward on recovery
    - _Requirements: 18.1, 18.5, 18.6_

  - [x] 25.2 Write property tests for logging
    - **Property 35: Structured Log Format Completeness**
    - **Property 37: Log Buffer Bounded Overflow**
    - **Validates: Requirements 18.1, 18.6**

  - [x] 25.3 Implement Prometheus metrics and alerting
    - Expose Prometheus-compatible metrics: request counts, latency histograms (p50/p90/p95/p99), LLM token usage, CPU %, memory bytes
    - Implement configurable alerting for error rate thresholds, resource usage, agent unresponsiveness
    - Deliver alerts to configured notification channel
    - _Requirements: 18.2, 18.3_

  - [x] 25.4 Write property tests for alerting
    - **Property 36: Alert Rule Evaluation Correctness**
    - **Validates: Requirements 18.3**

- [x] 26. Deployment configuration
  - [x] 26.1 Create Docker Compose deployment
    - Create Dockerfile for agent container (core + modules)
    - Create Dockerfile for web UI container (static + API proxy)
    - Create docker-compose.yml with Nginx reverse proxy (TLS termination), agent, web UI, Prometheus
    - Configure volumes for data (SQLite, logs), config (TOML, keys), and workspace (project files)
    - _Requirements: 12.1_

  - [x] 26.2 Create systemd service unit for bare-metal deployment
    - Create systemd service unit file for the agent
    - Configure auto-restart on failure
    - Configure startup after network is available
    - _Requirements: 12.2_

  - [x] 26.3 Implement zero-downtime configuration reload
    - Support reload signal (SIGHUP) for settings that don't require restart (rate limits, display text, feature flags)
    - Document which settings require full restart (service bindings, DB connections, auth)
    - _Requirements: 12.6_

- [x] 27. Final checkpoint - Full platform integration
  - Ensure all components work together end-to-end: agent processes messages via LLM, executes tasks on schedule, coding module edits files and runs commands, all interfaces display results, security enforces boundaries, and deployment configs are valid. Ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP
- Each task references specific requirements for traceability
- Checkpoints ensure incremental validation
- Property tests validate universal correctness properties (37 total across the platform)
- Unit tests validate specific examples and edge cases
- The Rust workspace structure allows parallel development of independent crates
- React/Vite web interface is developed in parallel with backend API
- proptest is used for all property-based tests in Rust

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2", "1.3"] },
    { "id": 2, "tasks": ["1.4", "3.1"] },
    { "id": 3, "tasks": ["3.2", "3.3", "3.5"] },
    { "id": 4, "tasks": ["3.4", "3.6", "3.7", "3.9"] },
    { "id": 5, "tasks": ["3.8", "5.1"] },
    { "id": 6, "tasks": ["5.2", "5.4", "5.6"] },
    { "id": 7, "tasks": ["5.3", "5.5", "5.7", "7.1"] },
    { "id": 8, "tasks": ["7.2", "8.1"] },
    { "id": 9, "tasks": ["7.3", "8.2", "8.4"] },
    { "id": 10, "tasks": ["8.3", "8.5", "10.1"] },
    { "id": 11, "tasks": ["10.2", "10.4", "11.1"] },
    { "id": 12, "tasks": ["10.3", "12.1", "13.1"] },
    { "id": 13, "tasks": ["12.2", "12.3", "14.1"] },
    { "id": 14, "tasks": ["12.4", "14.2", "15.1"] },
    { "id": 15, "tasks": ["15.2", "15.3", "16.1"] },
    { "id": 16, "tasks": ["18.1", "19.1"] },
    { "id": 17, "tasks": ["18.2", "18.3", "19.2", "19.3"] },
    { "id": 18, "tasks": ["21.1", "22.1"] },
    { "id": 19, "tasks": ["21.2", "21.3"] },
    { "id": 20, "tasks": ["23.1", "23.2", "23.3"] },
    { "id": 21, "tasks": ["23.4", "23.5"] },
    { "id": 22, "tasks": ["25.1", "25.3"] },
    { "id": 23, "tasks": ["25.2", "25.4", "26.1", "26.2", "26.3"] }
  ]
}
```
