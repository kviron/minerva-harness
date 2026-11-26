#[derive(Debug, Clone)]
pub struct Limits {
    pub active_per_root: usize,
    pub children_per_root: usize,
    pub queued_per_child: usize,
    pub max_depth: u32,
    pub prompt_bytes: usize,
    pub summary_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            active_per_root: 4,
            children_per_root: 16,
            queued_per_child: 8,
            max_depth: 2,
            prompt_bytes: 32 * 1024,
            summary_bytes: 8 * 1024,
        }
    }
}
