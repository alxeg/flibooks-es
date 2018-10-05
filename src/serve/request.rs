#[derive(Debug, Deserialize, Clone)]
pub struct Author {
    pub author: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

pub fn default_limit() -> u32 {
    10
}
