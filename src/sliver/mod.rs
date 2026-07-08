pub mod connection;
pub mod events;
pub mod profiles;
pub mod proto;

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::db::models::SliverProfile;

pub struct SliverAdapter {
    pub connection: Arc<Mutex<Option<connection::SliverConnection>>>,
    pub active_profile: tokio::sync::watch::Receiver<Option<SliverProfile>>,
}
