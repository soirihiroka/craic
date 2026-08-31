use craic_platform::{UiEffectCompletion, UiEffectRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

string_id!(PageId);
string_id!(ActionId);
string_id!(WorkspaceId);
string_id!(SessionId);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PageDescriptor {
    pub id: &'static str,
    pub label: &'static str,
}

impl PageDescriptor {
    pub fn page_id(self) -> PageId {
        PageId::new(self.id)
    }
}

pub const PAGE_DESCRIPTORS: [PageDescriptor; 5] = [
    PageDescriptor {
        id: "changes",
        label: "Changes",
    },
    PageDescriptor {
        id: "history",
        label: "History",
    },
    PageDescriptor {
        id: "files",
        label: "Files",
    },
    PageDescriptor {
        id: "containers",
        label: "Containers",
    },
    PageDescriptor {
        id: "agents",
        label: "Agents",
    },
];

pub fn page_descriptor(page: &PageId) -> Option<&'static PageDescriptor> {
    PAGE_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == page.as_str())
}

#[derive(
    Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Generation(u64);

impl Generation {
    pub const INITIAL: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCommand {
    pub page: Option<PageId>,
    pub action: ActionId,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSelection {
    pub id: WorkspaceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefreshScope {
    Application,
    Workspace,
    Page(PageId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceCompletion {
    Succeeded {
        request_id: uuid::Uuid,
        generation: Generation,
        payload: Value,
    },
    Failed {
        request_id: uuid::Uuid,
        generation: Generation,
        message: String,
    },
    Cancelled {
        request_id: uuid::Uuid,
        generation: Generation,
    },
}

impl ServiceCompletion {
    pub fn generation(&self) -> Generation {
        match self {
            Self::Succeeded { generation, .. }
            | Self::Failed { generation, .. }
            | Self::Cancelled { generation, .. } => *generation,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppCommand {
    ActivatePage(PageId),
    RoutePageCommand(PageCommand),
    SelectWorkspace(WorkspaceSelection),
    Refresh(RefreshScope),
    CompleteUiEffect(UiEffectCompletion),
    ServiceCompleted(ServiceCompletion),
    ShutdownRequested,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge {
    pub text: String,
    pub attention: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationViewState {
    pub active_page: Option<PageId>,
    pub workspace: Option<WorkspaceSelection>,
    pub workspace_generation: Generation,
    pub badges: BTreeMap<PageId, Badge>,
    pub refreshing: Vec<RefreshScope>,
    pub shutting_down: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageViewState {
    pub title: String,
    pub badge: Option<Badge>,
    pub refreshing: bool,
    pub data: Value,
}

#[derive(Clone, Debug)]
pub enum UiEvent {
    ApplicationState(Arc<ApplicationViewState>),
    PageState {
        page: PageId,
        revision: u64,
        state: Arc<PageViewState>,
    },
    Effect(UiEffectRequest),
    ShutdownReady,
}
