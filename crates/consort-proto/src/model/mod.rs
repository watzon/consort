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
mod conversation;
mod knowledge;
mod machine;
mod routine;
mod session;
mod tag;
mod tool;
mod waiting;
mod work;

pub(crate) use account::Redacted;
pub use account::{
    AcpAccountSpec, AcpRegistryAgent, SIGN_IN_IDLE_EXPIRY_MS, SIGN_IN_RETENTION_MS, SecretEnv,
    SignIn, SignInMethod, SignInState,
};
pub use common::{
    BlobRef, HashParseError, ModelOptions, ModelSelection, OptionValue, Os, Sha256Hash,
    ValidationError, validate_relative_path,
};
pub use conversation::{
    Author, Channel, ChannelKind, ChannelRepo, Group, Message, MessageKind, Reaction, ReadMarker,
    Repo,
};
pub use knowledge::{
    ARTIFACTS_DIR, ChannelFile, ExpectedHash, FileRef, MEMORY_PATH_MAX_BYTES, MemoryActor,
    MemoryAuthor, MemoryNote, MemoryNoteSummary, MemoryScope, ScopeParseError, artifact_path,
    memory_note_entity_id, validate_memory_path,
};
pub use machine::{
    AuthState, Capabilities, Checkout, CheckoutInfo, Client, ClientKind, HarnessAccount, Inventory,
    LocalMcpServerInfo, ModelInfo, OptionChoice, OptionDescriptor, PushProvider, PushRegistration,
    Worker, WorkerLoad, WorkerPolicy, WorkerState,
};
pub use routine::{
    ROUTINE_MIN_INTERVAL_SECONDS, Routine, RoutineAction, RoutineRun, RoutineRunStatus,
    RoutineSchedule,
};
pub use session::{
    AssistantMessagePayload, COMMAND_OUTPUT_TAIL_BYTES, CommandPayload, ErrorPayload, FileChange,
    FileChangeKind, FileChangePayload, ItemKind, ItemPayload, ItemStatus, PlanPayload, PlanStep,
    PlanStepStatus, ReasoningPayload, Session, SessionItem, SessionRole, SessionState,
    SubagentPayload, ToolCallPayload, Usage, WebSearchPayload, tail_of,
};
pub use tag::{InvalidTag, TAG_MAX_CHARS, is_valid_tag, normalize_tags, validate_tag};
pub use tool::{ToolId, ToolState, ToolStatus};
pub use waiting::{
    Approval, ApprovalDetails, ApprovalKind, ApprovalStatus, DECISION_CONSEQUENCE_MAX_CHARS,
    DECISION_LABEL_MAX_CHARS, DECISION_MAX_OPTIONS, DECISION_MIN_OPTIONS,
    DECISION_QUESTION_MAX_CHARS, Decision, DecisionAnswer, DecisionOption, DecisionStatus,
    RequestOption, RequestOptionKind, validate_decision,
};
pub use work::{ApprovalMode, Isolation, PlacementRequest, PlacementRequire, Task, TaskCreator};
