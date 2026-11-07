//! Pure logic. Nothing in here touches the filesystem, processes or devices;
//! probes and backends feed data in and carry plans out.

pub mod bluetooth;
pub mod facts;
pub mod merge;
pub mod model;
pub mod paths;
pub mod plan;
pub mod redact;
pub mod select;
pub mod settings;
pub mod stack;
