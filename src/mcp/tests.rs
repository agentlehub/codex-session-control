use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, json};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::{
    app_server::{TESTED_CODEX_CLI_VERSION, TESTED_CODEX_VERSION},
    model::TurnItemsView,
};

use super::*;

mod support;
use support::*;

mod descendant_interrupt;
mod goal_matrix;
mod mutation_mapping;
mod outcome_unknown;
mod read_tools;
mod threads_wait;
mod timeout;
mod validation;
