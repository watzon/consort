//! Every entity in data-model.md, as the client view and the wire shape.
//!
//! The submodules follow the sections of data-model.md. Everything is
//! re-exported here, so `consort_proto::model::Task` works.
//!
//! Serde rules for entities: field names are snake_case. An empty optional
//! field serializes as `null`. A missing optional field or list reads as
//! empty. Unknown fields are ignored, so an older peer can read a newer
//! entity.

mod account;
mod common;
mod connector;
mod conversation;
mod knowledge;
mod machine;
mod preferences;
mod proactivity;
mod routine;
mod session;
mod skill;
mod tag;
mod tool;
mod waiting;
mod work;

pub(crate) use account::Redacted;
pub use account::{
    AcpAccountSpec, AcpRegistryAgent, SIGN_IN_IDLE_EXPIRY_MS, SIGN_IN_IN_SETTINGS,
    SIGN_IN_RETENTION_MS, SecretEnv, SignIn, SignInMethod, SignInState,
};
pub use common::{
    BlobRef, HashParseError, ModelOptions, ModelSelection, OptionValue, Os, Sha256Hash,
    ValidationError, validate_relative_path,
};
pub use connector::{
    ApprovalRule, AuthScheme, AuthSchemeKind, CATALOG_REFRESH_MS, CONNECT_ATTEMPT_TIMEOUT_MS,
    CONNECTION_HANDLE_MAX_CHARS, CONNECTION_NAME_MAX_CHARS, CallDecision, CallOutcome,
    ComposioStatus, ComposioUsage, Connection, ConnectionStatus, ConnectorCall, ConnectorCategory,
    ConnectorContextEntry, ConnectorService, ConnectorToolSummary, CredentialField, DisplayField,
    ToolApprovals, ToolKind, UseIn, canonical_json, connection_handle, render_connectors_block,
    unique_connection_handle, validate_connection_name,
};
pub use conversation::{
    Author, Channel, ChannelKind, ChannelRepo, Group, Message, MessageKind, Reaction, ReadMarker,
    Repo,
};
pub use knowledge::{
    ARTIFACTS_DIR, ChannelFile, ExpectedHash, FileRef, MEMORY_DELETED_KEEP_DAYS,
    MEMORY_PATH_MAX_BYTES, MemoryActor, MemoryAuthor, MemoryNote, MemoryNoteSummary, MemoryScope,
    ScopeParseError, artifact_path, memory_note_entity_id, validate_memory_path,
};
pub use machine::{
    AuthState, Capabilities, Checkout, CheckoutInfo, Client, ClientKind, ExtrasGroup,
    HarnessAccount, Inventory, LocalMcpServerInfo, ModelInfo, OptionChoice, OptionDescriptor,
    PROVIDER_EXTRAS_NAMES_MAX, PairedBy, ProviderExtras, PushProvider, PushRegistration, Worker,
    WorkerLoad, WorkerPolicy, WorkerState,
};
pub use preferences::{
    FAVORITE_FIELD_MAX_CHARS, FAVORITE_MODELS_MAX, FavoriteModel, PREFERENCES_ENTITY_ID,
    Preferences,
};
pub use proactivity::{
    CHECK_IN_BACKOFF_MAX_SECONDS, CHECK_IN_CHANNEL_SECONDS, CHECK_IN_MIN_SECONDS,
    CHECK_IN_NOTE_MAX_BYTES, CHECK_IN_NOTE_PATH, CHECK_IN_TUTTI_SECONDS, CHECK_IN_WATCH_OUTSIDE,
    FINDING_MAX_CHARS, FOLLOW_UP_MAX_DAYS, FOLLOW_UP_MIN_MINUTES, FOLLOW_UP_REASON_MAX_CHARS,
    FOLLOW_UPS_MAX_PER_CHANNEL, NOTIFY_CAP_DEFAULT, OFFER_DO_IT, OFFER_NEVER, OFFER_NOT_NOW,
    PROACTIVE_LOG_KEEP_DAYS, PROACTIVITY_ENTITY_ID, ProactiveKind, ProactiveMark, ProactiveOutcome,
    ProactiveRating, ProactiveTasks, ProactiveWake, ProactivityLevel, ProactivitySettings,
    QuietHours, RESEARCH_MEMORY_FOLDER, RESEARCH_OFFERS_MAX_PER_DAY,
    SILENT_CHECK_INS_BEFORE_BACKOFF, STALE_NEEDS_INPUT_MS, STALE_REVIEW_MS,
    SUGGESTION_LABEL_MAX_CHARS, SUGGESTIONS_MAX, SnoozeChoice, Suggestion, SuggestionAction,
    TURN_BUDGET_CHANNEL, TURN_BUDGET_TUTTI, validate_suggestions,
};
pub use routine::{
    ROUTINE_MIN_INTERVAL_SECONDS, Routine, RoutineAction, RoutineKind, RoutineOrigin, RoutineRun,
    RoutineRunStatus, RoutineSchedule,
};
pub use session::{
    AssistantMessagePayload, COMMAND_OUTPUT_TAIL_BYTES, CommandPayload, ErrorPayload, FileChange,
    FileChangeKind, FileChangePayload, ItemKind, ItemPayload, ItemStatus, PlanPayload, PlanStep,
    PlanStepStatus, ReasoningPayload, Session, SessionItem, SessionRole, SessionState,
    SubagentPayload, ToolCallPayload, Usage, WebSearchPayload, tail_of,
};
pub use skill::{
    AuditStatus, SKILL_DESCRIPTION_MAX_CHARS, SKILL_MANIFEST_FILE, SKILL_MAX_FILE_BYTES,
    SKILL_MAX_FILES, SKILL_MAX_TOTAL_BYTES, SKILL_NAME_MAX_CHARS, SKILL_READ_MAX_BYTES,
    SKILL_UPDATE_CHECK_MS, SKILL_UPLOAD_MAX_BYTES, Skill, SkillAudit, SkillBundle, SkillBundleRef,
    SkillContextEntry, SkillDirectoryEntry, SkillDirectoryKind, SkillDirectorySource,
    SkillFileChange, SkillFileChangeKind, SkillFileInfo, SkillSource, SkillUpdateState,
    render_skills_block, skill_content_hash, validate_skill_description, validate_skill_files,
    validate_skill_name,
};
pub use tag::{InvalidTag, TAG_MAX_CHARS, is_valid_tag, normalize_tags, validate_tag};
pub use tool::{ToolId, ToolState, ToolStatus};
pub use waiting::{
    Approval, ApprovalDetails, ApprovalKind, ApprovalStatus, ConnectServiceCard, ConnectorCard,
    DECISION_CONSEQUENCE_MAX_CHARS, DECISION_LABEL_MAX_CHARS, DECISION_MAX_OPTIONS,
    DECISION_MIN_OPTIONS, DECISION_QUESTION_MAX_CHARS, Decision, DecisionAnswer, DecisionOption,
    DecisionStatus, HUB_CARD_ALLOW, HUB_CARD_DENY, HUB_CARD_REQUEST_PREFIX, ManagementCard,
    RequestOption, RequestOptionKind, SignInCard, SkillInstallCard, hub_card_request_id,
    validate_decision,
};
pub use work::{ApprovalMode, Isolation, PlacementRequest, PlacementRequire, Task, TaskCreator};
