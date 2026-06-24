//! Kernel services: the command registry, the resolver chain, and the matcher
//! backend. Concrete structs configured by data; no registration machinery.

pub mod command;
pub mod matcher;
pub mod resolver;

pub use command::CommandRegistry;
pub use matcher::{MatcherBackend, SkimMatcher, rank_paths};
pub use resolver::ResolverChain;
