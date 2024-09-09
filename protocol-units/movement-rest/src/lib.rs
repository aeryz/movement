use anyhow::Error;
use aptos_api::Context;
use aptos_types::account_address::AccountAddress;
use aptos_types::account_config::AccountResource;
use aptos_types::aggregate_signature::AggregateSignature;
use aptos_types::block_info::BlockInfo;
use aptos_types::epoch_change::EpochChangeProof;
use aptos_types::ledger_info::{LedgerInfo, LedgerInfoWithSignatures};
use aptos_types::proof::TransactionInfoWithProof;
use aptos_types::state_proof::StateProof;
use aptos_types::state_store::state_key::StateKey;
use aptos_types::state_store::table::TableHandle;
use move_core_types::move_resource::MoveStructType;
use poem::listener::TcpListener;
use poem::{
	get, handler,
	middleware::Tracing,
	web::{Data, Path},
	EndpointExt, IntoResponse, Response, Route, Server,
};
use std::env;
use std::sync::Arc;
use tracing::info;

#[derive(Debug)]
pub struct MovementRest {
	/// The URL to bind the REST service to.
	pub url: String,
	pub context: Option<Arc<Context>>,
	// More fields to be added here, log verboisty, etc.
}

impl MovementRest {
	pub const MOVEMENT_REST_ENV_VAR: &'static str = "MOVEMENT_REST_URL";

	pub fn try_from_env(context: Option<Arc<Context>>) -> Result<Self, Error> {
		let url =
			env::var(Self::MOVEMENT_REST_ENV_VAR).unwrap_or_else(|_| "0.0.0.0:30832".to_string());
		Ok(Self { url, context })
	}

	pub async fn run_service(&self) -> Result<(), Error> {
		info!("Starting movement rest service at {}", self.url);
		let movement_rest = self.create_routes();
		Server::new(TcpListener::bind(&self.url)).run(movement_rest).await.unwrap();
		Ok(())
	}

	pub fn create_routes(&self) -> impl EndpointExt {
		Route::new()
			.at("/health", get(health))
			.at("/movement/v1/state-root-hash/:blockheight", get(state_root_hash))
			.at("/movement/v1/resource-proof/:key/:addr/:blockheight", get(resource_proof))
			.at("/movement/v1/state-proof/:blockheight", get(state_proof))
			.at("/movement/v1/account-proof/:addr/:blockheight", get(account_proof))
			.at("movement/v1/richard", get(richard))
			.data(self.context.as_ref().unwrap().clone())
			.with(Tracing)
	}
}

#[handler]
pub async fn health() -> Response {
	"OK".into_response()
}

#[handler]
pub async fn richard() -> Response {
	"Well Done".into_response()
}

#[handler]
pub async fn state_root_hash(
	Path(blockheight): Path<u64>,
	context: Data<&Arc<Context>>,
) -> Result<Response, anyhow::Error> {
	let latest_ledger_info = context.db.get_latest_ledger_info()?;
	let (_, end_version, _) = context.db.get_block_info_by_height(blockheight)?;
	tracing::info!("end_version: {}", end_version);
	let txn_with_proof = context.db.get_transaction_by_version(
		end_version,
		latest_ledger_info.ledger_info().version(),
		false,
	)?;
	tracing::info!("txn_with_proof: {:?}", txn_with_proof);
	let state_root_hash = txn_with_proof
		.proof
		.transaction_info
		.state_checkpoint_hash()
		.ok_or_else(|| anyhow::anyhow!("No state root hash found"))?;
	Ok(state_root_hash.to_hex_literal().into_response())
}

#[handler]
pub async fn resource_proof(
	Path((key, addr, blockheight)): Path<(String, AccountAddress, u64)>,
	context: Data<&Arc<Context>>,
) -> Result<Response, anyhow::Error> {
	let (_, end_version, _) = context.db.get_block_info_by_height(blockheight)?;

	let key = hex::decode(&key)?;
	let key = StateKey::table_item(&TableHandle(addr), &key);

	let resp = context.db.get_state_value_with_proof_by_version(&key, end_version)?;

	Ok(serde_json::to_string(&resp.1)?.into_response())
}

#[handler]
pub async fn state_proof(
	Path(blockheight): Path<u64>,
	context: Data<&Arc<Context>>,
) -> Result<Response, anyhow::Error> {
	#[derive(serde::Serialize, serde::Deserialize)]
	struct StateProofResponse {
		tx_index: u64,
		state_proof: StateProof,
		tx_proof: TransactionInfoWithProof,
	}

	let (_, end_version, block_event) = context.db.get_block_info_by_height(blockheight)?;

	let mut epoch_state = context.db.get_latest_epoch_state()?;
	epoch_state.epoch = block_event.epoch();

	let block_info = BlockInfo::new(
		block_event.epoch(),
		block_event.round(),
		block_event.hash()?,
		context.db.get_accumulator_root_hash(end_version)?,
		end_version,
		block_event.timestamp,
		Some(epoch_state),
	);

	let ledger_info = LedgerInfoWithSignatures::new(
		LedgerInfo::new(block_info, Default::default()),
		AggregateSignature::empty(),
	);

	let state_proof = StateProof::new(ledger_info, EpochChangeProof::new(vec![], false));

	let tx = context.db.get_transaction_by_version(end_version, end_version, false)?;

	let tx_proof = tx.proof;

	let tx_index = tx.version;

	Ok(serde_json::to_string(&StateProofResponse { tx_index, state_proof, tx_proof })?
		.into_response())
}

#[handler]
pub async fn account_proof(
	Path((addr, blockheight)): Path<(AccountAddress, u64)>,
	context: Data<&Arc<Context>>,
) -> Result<Response, anyhow::Error> {
	let (_, end_version, _) = context.db.get_block_info_by_height(blockheight)?;

	let key = StateKey::resource(&addr, &<AccountResource as MoveStructType>::struct_tag())?;

	let resp = context.db.get_state_value_with_proof_by_version(&key, end_version)?;

	Ok(format!("{resp:?}").into_response())
}

#[cfg(test)]
mod tests {
	use super::*;
	use poem::test::TestClient;

	#[tokio::test]
	async fn test_health_endpoint() {
		let rest_service = MovementRest::try_from_env(None).expect("Failed to create MovementRest");
		assert_eq!(rest_service.url, "http://0.0.0.0:30832");
		// Create a test client
		let client = TestClient::new(rest_service.create_routes());

		// Test the /health endpoint
		let response = client.get("/health").send().await;
		assert!(response.0.status().is_success());
	}
}
