use super::model_window::CatalogModel;
use rig_reqwest::reqwest;
use serde::Deserialize;

pub(super) const BASE_URL: &str = "https://anymodel.org/v1";

#[derive(Deserialize)]
struct Catalog {
    data: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    kind: Option<String>,
    #[serde(alias = "context_window")]
    context_length: Option<u32>,
}

impl Catalog {
    fn chat_models(self) -> Vec<CatalogModel> {
        self.data
            .into_iter()
            .filter(|entry| matches!(entry.kind.as_deref(), None | Some("imageToText")))
            .map(|entry| CatalogModel { id: entry.id, context_window: entry.context_length.filter(|n| *n > 0) })
            .collect()
    }
}

pub(super) async fn discover(key: &str) -> Result<Vec<CatalogModel>, reqwest::Error> {
    let catalog = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get(format!("{BASE_URL}/models"))
        .bearer_auth(key)
        .send()
        .await?
        .error_for_status()?
        .json::<Catalog>()
        .await?;
    Ok(catalog.chat_models())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keeps_chat_routes_with_their_prefixes() {
        let catalog: Catalog = serde_json::from_str(
            r#"{"data":[
                {"id":"am/gpt-5.6-terra","context_length":128000},
                {"id":"am/image","kind":"image"},
                {"id":"am/embedding","kind":"embedding"},
                {"id":"am/vision","kind":"imageToText"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            catalog.chat_models(),
            [
                CatalogModel { id: "am/gpt-5.6-terra".into(), context_window: Some(128000) },
                CatalogModel { id: "am/vision".into(), context_window: None },
            ]
        );
    }
}
