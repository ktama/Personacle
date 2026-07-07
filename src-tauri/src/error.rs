use serde::ser::SerializeStruct;

/// アプリ全体のエラー分類 (設計8章)。フロントには kind + message で渡す。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Connection(String),
    #[error("{0}")]
    Generation(String),
    #[error("{0}")]
    Data(String),
    /// ペルソナが他セッション参加中 (EC-08)
    #[error("{0}")]
    Busy(String),
    #[error("{0}")]
    NotFound(String),
    /// 同名ペルソナ警告 (EC-04)。force=true で回避可能
    #[error("{0}")]
    DuplicateName(String),
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::Validation(_) => "validation",
            AppError::Connection(_) => "connection",
            AppError::Generation(_) => "generation",
            AppError::Data(_) => "data",
            AppError::Busy(_) => "busy",
            AppError::NotFound(_) => "not_found",
            AppError::DuplicateName(_) => "duplicate_name",
        }
    }
}

// Tauri コマンドの Err として返すために {kind, message} 形式で直列化する
impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Data(format!("データベースエラー: {e}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;
