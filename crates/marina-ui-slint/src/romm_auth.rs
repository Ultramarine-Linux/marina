use marina_romm::{Auth as RommAuth, Client as RommClient};

pub fn client(base_url: String, token: Option<&str>) -> RommClient {
    let client = RommClient::new(base_url);
    match token.filter(|token| !token.trim().is_empty()) {
        Some(token) => client.with_auth(RommAuth::Bearer(token.to_owned())),
        None => client,
    }
}
