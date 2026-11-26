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
}

impl Catalog {
    fn chat_models(self) -> Vec<String> {
        self.data
            .into_iter()
            .filter(|entry| matches!(entry.kind.as_deref(), None | Some("imageToText")))
            .map(|entry| entry.id)
            .collect()
    }
}

pub(super) async fn discover(key: &str) -> Result<Vec<String>, reqwest::Error> {
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
                {"id":"am/gpt-5.6-terra"},
                {"id":"am/image","kind":"image"},
                {"id":"am/embedding","kind":"embedding"},
                {"id":"am/vision","kind":"imageToText"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(catalog.chat_models(), ["am/gpt-5.6-terra", "am/vision"]);
    }
}
