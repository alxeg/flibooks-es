#[derive(Debug, Deserialize, Clone)]
pub struct Author {
    pub author: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Search {
    #[serde(default = "default_empty_string")]
    pub title: String,
    #[serde(default = "default_empty_string")]
    pub author: String,
    #[serde(default = "default_empty_string")]
    pub series: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default = "default_deleted")]
    pub deleted: bool,
    #[serde(default = "default_langs")]
    pub langs: Vec<String>,
}

pub fn default_limit() -> u32 {
    10
}

pub fn default_deleted() -> bool {
    false
}

pub fn default_empty_string() -> String {
    "".to_string()
}

pub fn default_langs() -> Vec<String> {
    Vec::new()
}
