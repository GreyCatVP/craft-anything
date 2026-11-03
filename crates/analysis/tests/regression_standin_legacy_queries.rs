use craft_anything_analysis::standin::{self, Answer, MatchKind, StandIn, Turns};
use serde_json::json;

fn saved_v1(first: &str, second: &str) -> StandIn {
    let saved = json!({
        "version": 1,
        "routes": [{
            "method": "GET", "template": "/feed",
            "samples": [
                {"path": "/feed", "query": first, "status": 200, "text": "first-page"},
                {"path": "/feed", "query": second, "status": 200, "text": "second-page"}
            ]
        }]
    });
    serde_json::from_str(&saved.to_string()).unwrap()
}

fn expect_answer(saved: &StandIn, query: &str, expected: &[u8], how: MatchKind) {
    let Answer::Found(found) = standin::answer(saved, &mut Turns::default(), "GET", "/feed", query, b"") else {
        panic!("the saved request should match");
    };
    assert_eq!(found.body, expected, "{query}");
    assert_eq!(found.how, how, "{query}");
}

#[test]
fn legacy_v1_encoded_credential_queries_select_the_matching_variant() {
    for name in ["%74oken", "a%70i_key", "ACCESS%5fTOKEN"] {
        let saved = saved_v1(&format!("{name}=recorded-fake&page=1"), &format!("{name}=recorded-fake&page=2"));
        // Ask for the second recording first: path-only fallback would return the first body.
        expect_answer(&saved, &format!("page=2&{name}=replacement-fake"), b"second-page", MatchKind::PathAndQuery);
        expect_answer(&saved, &format!("page=1&{name}=replacement-fake"), b"first-page", MatchKind::PathAndQuery);
    }
}

#[test]
fn already_scrubbed_and_ordinary_saved_queries_still_match() {
    for (first, second, request) in [
        ("%74oken=[dropped]&page=1", "%74oken=[dropped]&page=2", "page=2&%74oken=replacement-fake"),
        ("name=public&page=1", "name=public&page=2", "page=2&name=public"),
    ] {
        expect_answer(&saved_v1(first, second), request, b"second-page", MatchKind::PathAndQuery);
    }
}

#[test]
fn an_unrecorded_query_keeps_the_existing_path_fallback() {
    let saved = saved_v1("%74oken=recorded-fake&page=1", "%74oken=recorded-fake&page=2");
    expect_answer(&saved, "page=3&%74oken=replacement-fake", b"first-page", MatchKind::Path);
}
