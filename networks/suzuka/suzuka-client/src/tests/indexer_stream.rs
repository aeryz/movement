// pub mod alice_bob;
// pub mod indexer_stream;
// use std::str::FromStr;
// use url::Url;
use once_cell::sync::Lazy;
use aptos_protos::indexer::v1::{
    GetTransactionsRequest,
    raw_data_client::RawDataClient,
};
use futures::StreamExt;

static SUZUKA_CONFIG: Lazy<suzuka_config::Config> = Lazy::new(|| {
	let dot_movement = dot_movement::DotMovement::try_from_env().unwrap();
	let config = dot_movement.try_get_config_from_json::<suzuka_config::Config>().unwrap();
	config
});

static INDEXER_URL: Lazy<String> = Lazy::new(|| {
	let indexer_connection_hostname = SUZUKA_CONFIG
		.execution_config
		.maptos_config
		.client
		.maptos_indexer_grpc_connection_hostname
		.clone();
	let indexer_connection_port = SUZUKA_CONFIG
		.execution_config
		.maptos_config
		.client
		.maptos_indexer_grpc_connection_port
		.clone();

	let indexer_connection_url =
		format!("http://{}:{}", indexer_connection_hostname, indexer_connection_port);

	indexer_connection_url
});


#[tokio::test]
async fn test_example_indexer_stream() -> Result<(), anyhow::Error> {

    /*let channel = tonic::transport::Channel::from_shared(
        INDEXER_URL.to_string(),
    ).expect(
        "[Parser] Failed to build GRPC channel, perhaps because the data service URL is invalid",
    );*/
    
    let mut client = RawDataClient::connect(
        INDEXER_URL.as_str(),
    ).await?;

    let request = GetTransactionsRequest {
        starting_version : Some(0),
        transactions_count : Some(100),
        batch_size : Some(100),
    }; 

    let stream = client.get_transactions(request).await?;

    stream
	    .into_inner()
		.next()
		.await
		.ok_or(anyhow::anyhow!("No response from server"))??;

	Ok(())
}