use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAddParams {
    pub environment_id: String,
    pub exec_server_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
}

macro_rules! environment_id_params {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
            #[serde(rename_all = "camelCase")]
            pub struct $name {
                pub environment_id: String,
            }
        )+
    };
}

environment_id_params!(EnvironmentInfoParams, EnvironmentStatusParams);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentConnectionNotification {
    pub thread_id: String,
    pub environment_id: String,
}
