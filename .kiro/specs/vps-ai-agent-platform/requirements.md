# Requirements Document

## Introduction

VPS AI Agent Platform is a standalone, VPS-optimized AI agent system designed to run 24/7 on virtual private servers. The platform operates in two modes: a base "always-on agent" mode providing task automation, monitoring, and scheduled operations, and an optional "coding agent" mode that adds full software development capabilities including file editing, terminal access, and git integration. The platform is a completely new project — not a fork or extension of OpenClaw or OpenCode — though it draws inspiration from both.

## Glossary

- **Platform**: The VPS AI Agent Platform system as a whole
- **Agent_Core**: The base agent runtime responsible for LLM communication, task execution, memory, and lifecycle management
- **Coding_Module**: The optional layer providing software development capabilities (file editing, terminal, git)
- **Task_Scheduler**: The subsystem responsible for scheduling and executing recurring or deferred tasks
- **Session_Manager**: The component managing agent sessions, context windows, and conversation persistence
- **Tool_Registry**: The registry that manages available tools and their permissions
- **Plugin_System**: The extensibility framework allowing third-party plugins to add capabilities
- **Web_Interface**: The web-based UI for interacting with and monitoring the agent
- **TUI**: The terminal user interface for local/SSH-based interaction
- **API_Server**: The HTTP/WebSocket API for programmatic access to the platform
- **Config_Manager**: The component responsible for loading, validating, and managing configuration
- **Security_Layer**: The subsystem handling authentication, authorization, sandboxing, and audit logging
- **Memory_Store**: The persistent storage system for agent memory, conversation history, and learned context
- **LLM_Router**: The component responsible for routing requests to configured LLM providers and managing fallbacks
- **Process_Supervisor**: The component managing long-running processes, health checks, and automatic restarts
- **Messaging_Integration**: The subsystem providing connectivity to third-party messaging platforms (Telegram, Discord, WhatsApp)
- **Diff_Renderer**: The component responsible for generating visual diff images from file change records

## Requirements

### Requirement 1: LLM Provider Support

**User Story:** As a platform operator, I want to connect multiple LLM providers, so that I can choose the best model for each task and have fallback options.

#### Acceptance Criteria

1. THE Platform SHALL send prompt requests to and receive completion responses from OpenAI-compatible API endpoints configured as LLM providers
2. THE Platform SHALL send prompt requests to and receive completion responses from Anthropic Claude API configured as an LLM provider
3. THE Platform SHALL send prompt requests to and receive completion responses from local LLM inference via Ollama configured as an LLM provider
4. WHEN an LLM provider fails to respond within the configured timeout (configurable between 5 and 120 seconds per provider, default 30 seconds), THE LLM_Router SHALL route the request to the next configured fallback provider according to the operator-defined priority ordering
5. IF all configured fallback providers have been attempted and none returned a successful response, THEN THE LLM_Router SHALL return an error indication to the caller specifying that no providers are available, and SHALL NOT retry providers already attempted for that request
6. THE Config_Manager SHALL allow operators to configure between 1 and 10 LLM providers, each with per-provider API keys, model selections, and priority ordering that determines the fallback sequence
7. WHEN a request is routed to a fallback provider, THE LLM_Router SHALL log the failover event including the original provider, failure reason, and fallback target

### Requirement 2: Base Agent Mode — Always-On Operation

**User Story:** As a platform operator, I want the agent to run continuously on my VPS, so that it can perform automated tasks, respond to events, and maintain persistent context without manual intervention.

#### Acceptance Criteria

1. THE Agent_Core SHALL maintain operation continuously without requiring user interaction, targeting a minimum uptime of 99.5% measured over any rolling 7-day period excluding host-level outages
2. WHEN the host system restarts, THE Process_Supervisor SHALL automatically restart the agent within 30 seconds of the operating system reaching its default run level and restore the previous session state including active task queue, conversation history, and scheduler configuration
3. THE Agent_Core SHALL persist conversation history and learned context across restarts using the Memory_Store, where learned context includes user preferences, task configurations, and accumulated decision history
4. WHILE operating in base agent mode with no tasks actively executing, THE Agent_Core SHALL consume no more than 512MB of RAM and no more than 5% of a single CPU core
5. THE Task_Scheduler SHALL execute scheduled tasks at their configured times with a maximum drift of 60 seconds
6. WHEN a scheduled task fails, THE Task_Scheduler SHALL retry the task up to 3 times with exponential backoff starting at a 10-second base interval and capped at a maximum delay of 5 minutes between retries, and log each failure
7. IF the Agent_Core process terminates unexpectedly, THEN THE Process_Supervisor SHALL restart the agent within 10 seconds and restore the previous session state
8. IF a scheduled task fails after all 3 retry attempts are exhausted, THEN THE Task_Scheduler SHALL mark the task as failed, log a summary of all attempts, and continue executing subsequent scheduled tasks without interruption

### Requirement 3: Task Automation and Scheduling

**User Story:** As a platform operator, I want to define automated tasks with schedules and triggers, so that the agent can perform routine work without manual prompting.

#### Acceptance Criteria

1. THE Task_Scheduler SHALL support cron-style schedule expressions for recurring tasks with a minimum scheduling interval of 1 minute
2. THE Task_Scheduler SHALL support event-driven triggers including file system changes (creation, modification, or deletion of watched paths), webhook receipts, and time-based conditional expressions
3. WHEN a task is triggered, THE Agent_Core SHALL execute the task using the configured LLM and available tools
4. IF a triggered task fails during execution due to an LLM error, tool unavailability, or unhandled exception, THEN THE Task_Scheduler SHALL record the failure reason in the task history log and mark the task run status as "failed"
5. THE Task_Scheduler SHALL support task dependencies where one task's successful completion triggers a dependent task, with a maximum dependency chain depth of 10 tasks
6. IF a task dependency cycle is detected during task definition, THEN THE Task_Scheduler SHALL reject the task configuration and indicate which tasks form the cycle
7. THE Platform SHALL provide a task history log showing start time, end time, duration, run status (succeeded, failed, or timed-out), and error details for each task run, retaining entries for at least 30 days
8. WHEN a task exceeds its configured timeout (default: 300 seconds if not specified), THE Task_Scheduler SHALL terminate the task and record a timeout failure in the task history log

### Requirement 4: Coding Module — File Operations

**User Story:** As a developer, I want the agent to read, create, edit, and delete files on the VPS, so that it can assist with software development tasks.

#### Acceptance Criteria

1. WHERE the Coding_Module is enabled, THE Agent_Core SHALL provide file read, create, edit, and delete operations as available tools
2. WHEN a file edit is requested, THE Coding_Module SHALL create the edit using precise string replacement or full file write operations
3. WHEN a file operation targets a path that resolves outside the configured workspace directories including resolution of symbolic links and relative path components, THE Security_Layer SHALL reject the operation and log the attempt
4. THE Coding_Module SHALL support operations on text files of any size up to 10MB
5. WHEN a file is modified, THE Coding_Module SHALL record the change in an undo history allowing rollback of the last 50 file-level operations where each create, edit, or delete counts as one operation
6. IF a string replacement target is not found in the specified file, THEN THE Coding_Module SHALL reject the edit and return an error indicating the match string was not found
7. IF a file operation targets a file that exceeds 10MB, THEN THE Coding_Module SHALL reject the operation and return an error indicating the file size limit has been exceeded

### Requirement 5: Coding Module — Terminal Access

**User Story:** As a developer, I want the agent to execute shell commands on the VPS, so that it can run builds, tests, and other development tools.

#### Acceptance Criteria

1. WHERE the Coding_Module is enabled, THE Agent_Core SHALL provide shell command execution as an available tool
2. WHEN a shell command is executed, THE Coding_Module SHALL capture stdout, stderr, and the exit code and return them to the caller upon process completion
3. WHEN a shell command exceeds the configured timeout (default: 300 seconds), THE Coding_Module SHALL terminate the process and return a timeout error indicating the command that was terminated and the elapsed duration
4. THE Security_Layer SHALL enforce a configurable command allowlist and blocklist for shell execution, defaulting to deny-all when no allowlist is configured
5. IF a shell command matches a blocklist entry or is not present on the allowlist, THEN THE Security_Layer SHALL reject the command before execution and return an error indicating the command was denied
6. WHILE a shell command is executing, THE Coding_Module SHALL stream output to the requesting interface with no more than 2 seconds of latency between process output and delivery
7. THE Coding_Module SHALL support concurrent execution of up to 5 shell processes
8. IF a shell command execution is requested while 5 processes are already running, THEN THE Coding_Module SHALL reject the request and return an error indicating the concurrency limit has been reached

### Requirement 6: Coding Module — Git Integration

**User Story:** As a developer, I want the agent to perform git operations, so that it can manage version control as part of development workflows.

#### Acceptance Criteria

1. WHERE the Coding_Module is enabled, THE Coding_Module SHALL provide git operations including clone, pull, push, commit, branch, merge, and diff
2. WHEN creating a commit, THE Coding_Module SHALL generate a commit message that includes a summary line of at most 72 characters describing the type of change and the affected component, followed by a body listing the specific files or areas modified
3. IF a git push fails due to remote conflicts, THEN THE Coding_Module SHALL report the conflict to the operator including the conflicting branch name and affected file paths, rather than force-pushing
4. THE Coding_Module SHALL support authentication via SSH keys and personal access tokens for git remotes
5. IF a git operation is attempted on a directory outside the configured repository directories, THEN THE Coding_Module SHALL reject the operation and return an error message indicating the directory is not an authorized repository path
6. IF authentication with a git remote fails, THEN THE Coding_Module SHALL report the authentication failure to the operator including the remote name, and SHALL NOT retry authentication without updated credentials
7. WHEN performing git operations, THE Coding_Module SHALL operate only within configured repository directories

### Requirement 7: User Interface — Web Interface

**User Story:** As a platform operator, I want a web-based interface to interact with and monitor the agent, so that I can manage it from any device with a browser.

#### Acceptance Criteria

1. THE Web_Interface SHALL provide a chat interface for conversing with the agent, where sent messages are displayed within 2 seconds of submission and received agent responses are displayed within 2 seconds of generation
2. THE Web_Interface SHALL display agent status including current task, resource usage, and uptime, refreshed at intervals no greater than 5 seconds
3. THE Web_Interface SHALL provide a dashboard showing scheduled tasks, the 50 most recent activity entries, and system health indicators including CPU usage percentage, memory usage percentage, and service availability
4. WHEN the WebSocket connection is lost, THE Web_Interface SHALL automatically attempt reconnection and resynchronize state within 10 seconds
5. IF the WebSocket connection cannot be re-established within 10 seconds, THEN THE Web_Interface SHALL display a persistent connection-failure indicator and retry every 30 seconds until the connection is restored
6. THE Web_Interface SHALL be accessible over HTTPS with TLS 1.2 or higher
7. THE Web_Interface SHALL support responsive layouts for viewports ranging from 320px to 2560px wide, with a mobile-to-desktop breakpoint at 768px

### Requirement 8: User Interface — Terminal UI

**User Story:** As a platform operator, I want a terminal-based interface for interacting with the agent over SSH, so that I can manage the agent directly on the VPS without a browser.

#### Acceptance Criteria

1. THE TUI SHALL provide a chat interface that includes a scrollable message history area displaying at least the most recent 200 messages and a text input area for composing messages to the agent
2. THE TUI SHALL display agent status (idle, working, or error), current task progress as a percentage or step count with descriptive label, and resource usage including CPU percentage and memory usage in megabytes, refreshed at least once every 2 seconds
3. THE TUI SHALL support keyboard shortcuts for common operations including task cancellation, history navigation, and mode switching, with at least 3 distinct shortcuts discoverable via a help shortcut
4. WHEN the terminal window is resized, THE TUI SHALL adapt its layout to the new dimensions within 100 milliseconds
5. WHILE connected over SSH with latency up to 500 milliseconds, THE TUI SHALL render updates without corrupting the display layout and accept user input without dropping characters
6. THE TUI SHALL support terminal dimensions of 80 columns by 24 rows or larger
7. IF the connection to the agent is lost, THEN THE TUI SHALL display a notification indicating the disconnection and attempt to reconnect automatically at intervals of no more than 5 seconds for up to 6 attempts

### Requirement 9: User Interface — API Access

**User Story:** As a developer, I want a programmatic API to interact with the agent, so that I can integrate it with external tools and automation systems.

#### Acceptance Criteria

1. THE API_Server SHALL expose a RESTful HTTP API for task submission, status queries, and configuration management
2. THE API_Server SHALL expose a WebSocket endpoint for real-time streaming of agent responses
3. THE API_Server SHALL require authentication via API keys (minimum 32 characters) for all endpoints
4. THE API_Server SHALL return structured JSON responses with consistent error formats including an error code, human-readable message, and request identifier
5. WHEN an API request fails validation, THE API_Server SHALL return a 400 status code with a description of the validation error
6. THE API_Server SHALL enforce configurable rate limits per API key, defaulting to 100 requests per minute
7. IF an API key exceeds its configured rate limit, THEN THE API_Server SHALL return a 429 status code with a Retry-After header indicating when the client may retry

### Requirement 10: Security — Authentication and Authorization

**User Story:** As a platform operator, I want robust security controls, so that the agent and its interfaces are protected from unauthorized access.

#### Acceptance Criteria

1. IF a request to the Web_Interface, TUI, or API_Server does not include valid authentication credentials, THEN THE Security_Layer SHALL reject the request and return an error indicating that authentication is required, without revealing whether the targeted resource exists
2. THE Security_Layer SHALL support API key authentication for programmatic access, where API keys are at least 32 characters in length
3. THE Security_Layer SHALL support username/password authentication with bcrypt-hashed passwords for interactive access, requiring passwords to be at least 12 characters in length
4. WHEN 5 consecutive failed authentication attempts occur from the same IP address within a 10-minute window, THE Security_Layer SHALL block that IP for 15 minutes and log the block event
5. THE Security_Layer SHALL log all authentication attempts including timestamp, source IP, username or API key identifier, and outcome (success or failure)
6. THE Security_Layer SHALL enforce role-based access control with at minimum "admin" and "operator" roles, where "admin" can manage users, roles, and all system configuration, and "operator" can execute agent tasks and view logs but cannot modify user accounts or security settings
7. WHEN an authenticated session has been inactive for more than 30 minutes, THE Security_Layer SHALL invalidate the session and require re-authentication

### Requirement 11: Security — Sandboxing and Resource Limits

**User Story:** As a platform operator, I want the agent's operations to be sandboxed and resource-limited, so that a misbehaving agent cannot damage the host system.

#### Acceptance Criteria

1. THE Security_Layer SHALL enforce configurable filesystem access boundaries restricting agent operations to designated directories, with separate controls for read, write, and execute permissions per directory
2. THE Security_Layer SHALL enforce configurable CPU and memory limits for agent-spawned processes, with memory limits specified in megabytes and CPU limits specified as a percentage of a single core
3. THE Security_Layer SHALL enforce network access controls with a default-deny policy, restricting which hosts and ports agent-spawned processes can reach to an explicitly configured allowlist
4. WHEN a resource limit is exceeded, THE Security_Layer SHALL terminate the offending process within 5 seconds and log the violation including the process identifier, the resource type exceeded, and the measured value at termination
5. IF container or namespace isolation is available on the host, THEN THE Platform SHALL run agent-spawned processes inside that isolation boundary
6. IF an agent-spawned process attempts to access a filesystem path outside designated directories, THEN THE Security_Layer SHALL deny the operation and return an error indicating the access was blocked by sandbox policy
7. IF an agent-spawned process attempts to reach a network host or port not on the configured allowlist, THEN THE Security_Layer SHALL block the connection and log the denied destination host and port

### Requirement 12: Deployment and Operations

**User Story:** As a platform operator, I want simple deployment options for my VPS, so that I can get the platform running quickly and keep it updated.

#### Acceptance Criteria

1. THE Platform SHALL provide a Docker Compose deployment configuration as the primary deployment method
2. THE Platform SHALL provide a systemd service unit file for bare-metal deployment
3. THE Platform SHALL support configuration via environment variables and TOML configuration files, where environment variables take precedence over TOML file values for any setting defined in both sources
4. WHEN the platform starts, THE Config_Manager SHALL validate all configuration settings and report each invalid setting with the setting name and reason for failure, and SHALL refuse to start until all configuration errors are resolved
5. THE Platform SHALL provide a health check endpoint that responds within 2 seconds, returning the platform version and a status of healthy or unhealthy based on connectivity to required backing services
6. THE Platform SHALL support zero-downtime configuration reloads via a reload signal for settings that do not require service restart, such as rate limits, display text, and feature flags, while settings that affect service bindings, database connections, or authentication require a full restart

### Requirement 13: Plugin and Extension System

**User Story:** As a developer, I want to extend the platform with custom plugins, so that I can add new tools, integrations, and capabilities without modifying the core.

#### Acceptance Criteria

1. THE Plugin_System SHALL support loading plugins from a configurable plugins directory at startup, discovering all plugins that provide a valid plugin manifest declaring the plugin name, version, and required API version
2. WHEN a plugin file is added, modified, or removed in the plugins directory, THE Plugin_System SHALL detect the change and reload the affected plugin within 10 seconds without restarting the agent, while allowing any in-flight operations from the previous plugin version to complete or timeout within 30 seconds
3. THE Plugin_System SHALL provide a defined API for plugins to register new tools with the Tool_Registry
4. THE Plugin_System SHALL provide a defined API for plugins to register new event handlers with the Task_Scheduler
5. WHEN a plugin fails to load due to an invalid manifest, missing dependencies, or runtime initialization error, THE Plugin_System SHALL log the error including the plugin name and failure reason, and continue loading remaining plugins without affecting agent operation
6. THE Plugin_System SHALL enforce the same resource limits and permission boundaries configured in the Security_Layer for plugin-spawned operations, including filesystem access boundaries, CPU and memory limits, and network access controls
7. IF a plugin attempts to register a tool or event handler with a name already registered by another plugin, THEN THE Plugin_System SHALL reject the registration, log a conflict error identifying both plugins, and continue operating with the original registration intact

### Requirement 14: Memory and Context Management

**User Story:** As a platform operator, I want the agent to maintain persistent memory and context, so that it can learn from past interactions and maintain continuity across sessions.

#### Acceptance Criteria

1. THE Memory_Store SHALL persist conversation history across agent restarts, retaining at least the most recent 1000 messages per session
2. THE Session_Manager SHALL support at least 50 concurrent conversation sessions with independent context, where messages and state in one session do not affect another session unless explicitly shared via stored knowledge
3. THE Memory_Store SHALL support semantic search over past conversations and stored knowledge, returning ranked results within 2 seconds for stores containing up to 100,000 entries
4. WHEN the context window token usage exceeds 80% of the configured LLM's token limit, THE Session_Manager SHALL summarize older context to maintain continuity while staying within the token limit, preserving all explicitly stored knowledge and the most recent 10 conversation turns in full
5. THE Memory_Store SHALL support explicit knowledge storage where the operator can save facts, preferences, and instructions that persist indefinitely, with each entry supporting up to 10,000 characters of content
6. THE Memory_Store SHALL provide an API for querying and managing stored memories including deletion of specific entries, where deletion is permanent and the entry is no longer retrievable via search or direct query
7. IF the Memory_Store is unavailable during a session, THEN THE Session_Manager SHALL continue operating using in-memory context for the current session and SHALL retry persistence at intervals of 30 seconds until the Memory_Store becomes available
8. IF the Memory_Store reaches its configured storage capacity, THEN THE Memory_Store SHALL reject new entries with an error indicating storage is full, without deleting or overwriting existing entries

### Requirement 15: Coding Module — LSP Integration

**User Story:** As a developer, I want the agent to leverage Language Server Protocol support, so that it can provide intelligent code assistance including diagnostics, completions, and refactoring awareness.

#### Acceptance Criteria

1. WHERE the Coding_Module is enabled, THE Coding_Module SHALL initialize and maintain connections to all Language Server Protocol servers configured for the current workspace's languages
2. WHEN a file is modified, THE Coding_Module SHALL request updated diagnostics from the relevant language server within 2 seconds and, IF diagnostics contain errors, THEN THE Coding_Module SHALL report the errors to the operator and withhold the changes until the operator acknowledges or the errors are resolved
3. WHEN performing a refactoring operation, THE Coding_Module SHALL use LSP capabilities including go-to-definition, find-references, and rename-symbol to identify and update all references to the affected symbol across the workspace
4. WHEN a file is modified, THE Coding_Module SHALL request updated diagnostics from the relevant language server within 2 seconds
5. THE Config_Manager SHALL allow operators to configure which language servers are available and their initialization options per language
6. IF a configured language server fails to start or terminates unexpectedly during operation, THEN THE Coding_Module SHALL log the failure, notify the operator, and continue operating without LSP features for that language until the server is restarted
7. IF no language server is configured for a file's language, THEN THE Coding_Module SHALL proceed with file operations without LSP diagnostics or refactoring assistance

### Requirement 16: Coding Module — Change Visibility and Diff Rendering

**User Story:** As a developer, I want to clearly see what the AI agent changed in my files, so that I can review modifications before accepting them and maintain awareness of all changes.

#### Acceptance Criteria

1. WHERE the Coding_Module is enabled, THE Coding_Module SHALL track all file modifications made during a session with before and after states, where a session begins when the user initiates a coding task and ends when the user explicitly closes the task or starts a new one
2. WHEN a file is modified, THE Coding_Module SHALL generate a unified diff showing added, removed, and modified lines with at least 3 surrounding context lines above and below each change
3. THE Coding_Module SHALL render diffs as images including the file name, line numbers, added lines highlighted in green, and removed lines highlighted in red
4. WHEN a file change is made, THE Web_Interface SHALL display the corresponding diff image inline in the chat interface
5. WHEN a session ends or when the user requests a summary, THE Coding_Module SHALL provide a session change summary listing all modified files with total lines added and removed
6. WHEN requested, THE Coding_Module SHALL generate a combined diff image showing all changes made across multiple files in a single operation
7. IF a file is modified more than once during a session, THEN THE Coding_Module SHALL track each modification independently and display the cumulative diff relative to the original file state
8. IF the Coding_Module encounters a binary or non-text file modification, THEN THE Coding_Module SHALL display an indication that the file was changed without rendering a line-level diff

### Requirement 17: Third-Party Messaging Integration

**User Story:** As a platform operator, I want to interact with the agent through third-party messaging apps, so that I can manage and communicate with the agent from familiar platforms without needing the web interface.

#### Acceptance Criteria

1. THE Platform SHALL support integration with Telegram bots as a messaging interface
2. THE Platform SHALL support integration with Discord bots as a messaging interface
3. THE Platform SHALL support integration with WhatsApp bots as a messaging interface
4. WHEN a message is received on a connected messaging platform, THE Agent_Core SHALL process the message as a conversation input and respond on the same platform within 30 seconds
5. THE messaging integrations SHALL support management commands including task scheduling, status queries, mode switching, and configuration changes, where each command returns a confirmation or result message to the operator on the same platform
6. WHEN the Coding_Module generates a diff image, THE messaging integrations SHALL send the diff image inline with the response message, or IF the platform does not support inline images, THEN THE messaging integrations SHALL send the image as an attachment
7. THE Config_Manager SHALL allow operators to configure which messaging platforms are active and their authentication credentials, and SHALL validate credentials upon save by attempting a test connection to the platform
8. THE messaging integrations SHALL map each messaging platform user identity (Telegram user ID, Discord user ID, or WhatsApp phone number) to a platform operator account and enforce the same role-based authorization rules as the Web_Interface
9. IF a message is received from a messaging platform user whose identity is not mapped to an authorized operator account, THEN THE messaging integrations SHALL reject the message and respond with an error message indicating the user is not authorized
10. IF a connected messaging platform becomes unreachable when sending a response, THEN THE messaging integrations SHALL retry delivery up to 3 times with a 5-second interval, and IF all retries fail, THEN THE system SHALL log the failure and discard the pending response

### Requirement 18: Monitoring and Observability

**User Story:** As a platform operator, I want comprehensive monitoring and logging, so that I can understand agent behavior, diagnose issues, and track resource usage.

#### Acceptance Criteria

1. THE Platform SHALL emit structured logs in JSON format with configurable log levels (DEBUG, INFO, WARN, ERROR, FATAL), where each log entry includes at minimum a timestamp, correlation ID, log level, source component name, and message field
2. THE Platform SHALL expose Prometheus-compatible metrics including request counts, latency histograms (p50, p90, p95, p99), LLM token usage (prompt tokens, completion tokens, total tokens), CPU usage percentage, and memory usage in bytes
3. THE Platform SHALL provide configurable alerting for error rate thresholds, resource usage exceeding a configured percentage of allocated capacity, and agent unresponsiveness, delivering alert notifications to at least one configured notification channel
4. WHEN the agent has been unresponsive for more than 60 seconds, THE Process_Supervisor SHALL restart the agent and log the incident
5. THE Platform SHALL retain logs for a configurable duration (default 30 days, minimum 1 day, maximum 365 days) with automatic rotation when a single log file exceeds a configured maximum size, and cleanup of files older than the configured retention period
6. IF the logging or metrics subsystem becomes unavailable, THEN THE Platform SHALL buffer up to 1000 log entries in memory and resume forwarding when the subsystem recovers, discarding oldest entries first if the buffer is full
