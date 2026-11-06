//! `ytcfg.set({...})` object scraped from the watch page.

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct YtCfg {
    #[serde(rename = "DELEGATED_SESSION_ID", default)]
    pub delegated_session_id: String,
    #[serde(rename = "ID_TOKEN", default)]
    pub id_token: String,
    #[serde(rename = "HL", default)]
    pub hl: String,
    #[serde(rename = "INNERTUBE_API_KEY", default)]
    pub innertube_api_key: String,
    #[serde(rename = "INNERTUBE_CLIENT_NAME", default)]
    pub innertube_client_name: String,
    #[serde(rename = "INNERTUBE_CLIENT_VERSION", default)]
    pub innertube_client_version: String,
    #[serde(rename = "INNERTUBE_CONTEXT_CLIENT_NAME", default)]
    pub innertube_ctx_client_name: i64,
    #[serde(rename = "INNERTUBE_CONTEXT_CLIENT_VERSION", default)]
    pub innertube_ctx_client_version: String,
    #[serde(rename = "SESSION_INDEX", default)]
    pub session_index: String,
    #[serde(rename = "VISITOR_DATA", default)]
    pub visitor_data: String,
}
