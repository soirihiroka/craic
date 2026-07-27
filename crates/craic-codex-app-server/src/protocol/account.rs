use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoginAppBrand {
    #[default]
    Codex,
    Chatgpt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LoginAccountParams {
    #[serde(rename = "apiKey", rename_all = "camelCase")]
    ApiKey { api_key: String },
    #[serde(rename = "chatgpt", rename_all = "camelCase")]
    Chatgpt {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        codex_streamlined_login: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        use_hosted_login_success_page: bool,
        #[serde(default)]
        app_brand: Option<LoginAppBrand>,
    },
    #[serde(rename = "chatgptDeviceCode")]
    ChatgptDeviceCode,
    #[serde(rename = "chatgptAuthTokens", rename_all = "camelCase")]
    ChatgptAuthTokens {
        access_token: String,
        chatgpt_account_id: String,
        chatgpt_plan_type: Option<String>,
    },
    #[serde(rename = "amazonBedrock", rename_all = "camelCase")]
    AmazonBedrock { api_key: String, region: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelLoginAccountParams {
    pub login_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginCompletedNotification {
    pub login_id: Option<String>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccountParams {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refresh_token: bool,
}
