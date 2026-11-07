//! Pure logic. Nothing in here touches the filesystem, processes or devices;
//! probes and backends feed data in and carry plans out.

pub mod facts;
pub mod merge;
pub mod model;
pub mod plan;
pub mod redact;
