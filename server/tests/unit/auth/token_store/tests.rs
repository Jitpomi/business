use super::*;
#[test]
fn issuer_and_token_are_unambiguous() {
    assert_ne!(
        LocalTokenStore::key("ab", "c"),
        LocalTokenStore::key("a", "bc")
    );
}
