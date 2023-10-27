mod integration_tests {
  use serde::{Deserialize, Serialize};
  use serde_json::json;

  async fn initiate_test_server(mock_schema_path: String) -> &'static str {
    let config = tailcall::config::Config::from_file_paths([mock_schema_path].iter())
      .await
      .unwrap();
    tailcall::http::start_server(config)
      .await
      .expect("Server failed to start");
    "Success"
  }

  #[derive(Serialize, Deserialize, Debug)]
  struct Resp {
    pub data: Data,
  }

  #[derive(Serialize, Deserialize, Debug)]
  struct Data {
    pub post: Post,
  }

  #[derive(Serialize, Deserialize, Debug)]
  struct Post {
    pub title: String,
  }

  #[tokio::test]
  async fn test_batched_request() {
    let schema_path = "tests/graphql_mock/test-batched-request.graphql";

    tokio::spawn(initiate_test_server(schema_path.into()));
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    let http_client = reqwest::Client::new();

    let query_1_data = json!({"query": "query {post(id: 1) {title}}"});
    let query_2_data = json!({"query": "query { post(id: 2) { title } }"});
    let batched_query_data = format!("[{},{}]", query_1_data, query_2_data);

    let api_request = http_client
      .post("http://localhost:8000/graphql")
      .header("Content-Type", "application/json")
      .body(batched_query_data);

    let response = api_request.send().await.expect("Failed to send request");
    let json = response.json::<Vec<Resp>>().await.unwrap();

    assert_eq!(json.len(), 2);
    assert_eq!(
      json.get(0).unwrap().data.post.title,
      "sunt aut facere repellat provident occaecati excepturi optio reprehenderit"
    );
    assert_eq!(json.get(1).unwrap().data.post.title, "qui est esse");
  }
}
