use std::fmt;
use std::process::Command;

/// Oldest Codex CLI release supported by the native client.
pub const MINIMUM_CODEX_VERSION: CodexVersion = CodexVersion::new(0, 145, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodexVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl CodexVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Extracts the first semantic version triplet from output such as `codex-cli 0.145.0`.
    pub fn parse(output: &str) -> Result<Self, VersionError> {
        for token in output.split(|character: char| character.is_whitespace() || character == 'v') {
            let token = token
                .trim_matches(|character: char| !character.is_ascii_digit() && character != '.');
            let mut parts = token.split('.');
            let (Some(major), Some(minor), Some(patch)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let patch = patch
                .split(|character: char| !character.is_ascii_digit())
                .next()
                .unwrap_or_default();
            if parts.next().is_none()
                && let (Ok(major), Ok(minor), Ok(patch)) =
                    (major.parse(), minor.parse(), patch.parse())
            {
                return Ok(Self::new(major, minor, patch));
            }
        }

        Err(VersionError::Unrecognized(output.trim().to_owned()))
    }

    pub fn ensure_supported(self) -> Result<Self, VersionError> {
        if self < MINIMUM_CODEX_VERSION {
            return Err(VersionError::Unsupported {
                found: self,
                minimum: MINIMUM_CODEX_VERSION,
            });
        }
        Ok(self)
    }
}

impl fmt::Display for CodexVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug)]
pub enum VersionError {
    Command(std::io::Error),
    CommandFailed {
        status: Option<i32>,
        stderr: String,
    },
    Unrecognized(String),
    Unsupported {
        found: CodexVersion,
        minimum: CodexVersion,
    },
}

impl fmt::Display for VersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => write!(formatter, "failed to query the Codex version: {error}"),
            Self::CommandFailed { status, stderr } => write!(
                formatter,
                "Codex version command failed with status {status:?}: {}",
                stderr.trim()
            ),
            Self::Unrecognized(output) => {
                write!(formatter, "could not parse a Codex version from {output:?}")
            }
            Self::Unsupported { found, minimum } => write!(
                formatter,
                "Codex {found} is unsupported; install Codex {minimum} or newer"
            ),
        }
    }
}

impl std::error::Error for VersionError {}

/// Runs the supplied version command and enforces [`MINIMUM_CODEX_VERSION`].
pub fn check_codex_version(command: &mut Command) -> Result<CodexVersion, VersionError> {
    let output = command.output().map_err(VersionError::Command)?;
    if !output.status.success() {
        return Err(VersionError::CommandFailed {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    CodexVersion::parse(&String::from_utf8_lossy(&output.stdout))?.ensure_supported()
}
