mod actions;
mod command;
mod diff;
mod history;
mod remote;
mod repo;
mod types;

use crate::{bitbucket, github, gitlab};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(crate) use actions::*;
pub(crate) use command::*;
pub(crate) use diff::*;
pub use history::*;
pub(crate) use remote::*;
pub(crate) use repo::*;
pub use types::*;

const COMMIT_TIMEZONE_KEY: &str = "craic.commitTimezone";
const USE_SYSTEM_TIMEZONE_KEY: &str = "craic.useSystemTimezone";
const SHOW_REMOTE_OWNER_WARNING_KEY: &str = "craic.showRemoteOwnerWarning";
const DEFAULT_COMMIT_TIMEZONE: &str = "+0000";
pub(crate) const MAX_TEXT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
