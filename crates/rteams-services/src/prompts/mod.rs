//! Team prompt management (versioned lead / member prompts, injection policy).

pub mod policy;

pub use policy::{
    check_revision, configuration_revision, injected, validate_member_name, validate_prompt,
    PromptError, PromptHistory, PromptKey,
};
