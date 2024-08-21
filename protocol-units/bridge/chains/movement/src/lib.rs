use crate::utils::MovementAddress;
use anyhow::{Error, Result};
use aptos_sdk::{
	move_types::language_storage::TypeTag,
	rest_client::{Client, FaucetClient},
	types::LocalAccount,
};
use aptos_types::account_address::AccountAddress;
use bridge_shared::{
	bridge_contracts::{
		BridgeContractCounterparty, BridgeContractCounterpartyError,
		BridgeContractCounterpartyResult,
	},
	types::{
		Amount, BridgeTransferDetails, BridgeTransferId, HashLock, HashLockPreImage,
		InitiatorAddress, RecipientAddress, TimeLock,
	},
};
use rand::prelude::*;
use serde::Serialize;
use std::{env, io::{Write, Read}, process::{Command, Stdio}};
use std::str::FromStr;
use std::{
	sync::{mpsc, Arc, Mutex, RwLock},
	thread,
};
use tokio::{
	io::{AsyncBufReadExt, BufReader},
	process::Command as TokioCommand,
	sync::oneshot,
	task,
};

use url::Url;

pub mod utils;

const DUMMY_ADDRESS: AccountAddress = AccountAddress::new([0; 32]);
const COUNTERPARTY_MODULE_NAME: &str = "atomic_bridge_counterparty";

enum Call {
	Lock,
	Complete,
	Abort,
	GetDetails,
}

pub struct Config {
	pub rpc_url: Option<String>,
	pub ws_url: Option<String>,
	pub chain_id: String,
	pub signer_private_key: Arc<RwLock<LocalAccount>>,
	pub initiator_contract: Option<MovementAddress>,
	pub gas_limit: u64,
}

impl Config {
	pub fn build_for_test() -> Self {
		let seed = [3u8; 32];
		let mut rng = rand::rngs::StdRng::from_seed(seed);

		Config {
			rpc_url: Some("http://localhost:8080".parse().unwrap()),
			ws_url: Some("ws://localhost:8080".parse().unwrap()),
			chain_id: 4.to_string(),
			signer_private_key: Arc::new(RwLock::new(LocalAccount::generate(&mut rng))),
			initiator_contract: None,
			gas_limit: 10_000_000_000,
		}
	}
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct MovementClient {
	///Address of the counterparty moduke
	counterparty_address: AccountAddress,
	///Address of the initiator module
	initiator_address: Vec<u8>,
	///The Apotos Rest Client
	pub rest_client: Client,
	///The Apotos Rest Client
	pub faucet_client: Option<Arc<RwLock<FaucetClient>>>,
	///The signer account
	signer: Arc<LocalAccount>,
}

impl MovementClient {
	pub async fn new(config: Config) -> Result<Self, anyhow::Error> {
		let node_connection_url = format!("http://127.0.0.1:8080");
		let node_connection_url = Url::from_str(node_connection_url.as_str()).unwrap();

		let rest_client = Client::new(node_connection_url.clone());

		let seed = [3u8; 32];
		let mut rng = rand::rngs::StdRng::from_seed(seed);
		let signer = LocalAccount::generate(&mut rng);

		let mut address_bytes = [0u8; AccountAddress::LENGTH];
        	address_bytes[0..2].copy_from_slice(&[0xca, 0xfe]);
		let counterparty_address = AccountAddress::new(address_bytes);

		Ok(MovementClient {
			counterparty_address,
			initiator_address: Vec::new(), //dummy for now
			rest_client,
			faucet_client: None,
			signer: Arc::new(signer),
		})
	}

	pub async fn new_for_test(
		config: Config,
	) -> Result<(Self, tokio::process::Child), anyhow::Error> {
		let (setup_complete_tx, mut setup_complete_rx) = oneshot::channel();
		let mut child = TokioCommand::new("movement")
			.args(&["node", "run-local-testnet"])
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()?;

		let stdout = child.stdout.take().expect("Failed to capture stdout");
		let stderr = child.stderr.take().expect("Failed to capture stderr");

		let node_handle = task::spawn(async move {
			let mut stdout_reader = BufReader::new(stdout).lines();
			let mut stderr_reader = BufReader::new(stderr).lines();

			loop {
				tokio::select! {
					line = stdout_reader.next_line() => {
						match line {
							Ok(Some(line)) => {
								println!("STDOUT: {}", line);
								if line.contains("Setup is complete") {
									println!("Testnet is up and running!");
									let _ = setup_complete_tx.send(());
																	return Ok(());
								}
							},
							Ok(None) => {
								return Err(anyhow::anyhow!("Unexpected end of stdout stream"));
							},
							Err(e) => {
								return Err(anyhow::anyhow!("Error reading stdout: {}", e));
							}
						}
					},
					line = stderr_reader.next_line() => {
						match line {
							Ok(Some(line)) => {
								println!("STDERR: {}", line);
								if line.contains("Setup is complete") {
									println!("Testnet is up and running!");
									let _ = setup_complete_tx.send(());
																	return Ok(());
								}
							},
							Ok(None) => {
								return Err(anyhow::anyhow!("Unexpected end of stderr stream"));
							}
							Err(e) => {
								return Err(anyhow::anyhow!("Error reading stderr: {}", e));
							}
						}
					}
				}
			}
		});

		setup_complete_rx.await.expect("Failed to receive setup completion signal");
		println!("Setup complete message received.");

		let node_connection_url = format!("http://127.0.0.1:8080");
		let node_connection_url = Url::from_str(node_connection_url.as_str()).unwrap();
		let rest_client = Client::new(node_connection_url.clone());

		let faucet_url = format!("http://127.0.0.1:8081");
		let faucet_url = Url::from_str(faucet_url.as_str()).unwrap();
		let faucet_client = Arc::new(RwLock::new(FaucetClient::new(
			faucet_url.clone(),
			node_connection_url.clone(),
		)));

		let mut rng = ::rand::rngs::StdRng::from_seed([3u8; 32]);
		Ok((
			MovementClient {
				counterparty_address: DUMMY_ADDRESS,
				initiator_address: Vec::new(), // dummy for now
				rest_client,
				faucet_client: Some(faucet_client),
				signer: Arc::new(LocalAccount::generate(&mut rng)),
			},
			child,
		))
	}
	
	pub fn publish_for_test(&self) -> Result<()> {
		println!("Current directory: {:?}", env::current_dir());
		let mut process = Command::new("movement")
                .args(&["init"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("Failed to execute command");

		let stdin: &mut std::process::ChildStdin = process.stdin.as_mut().expect("Failed to open stdin");

		// Press enter for the first prompt
		stdin.write_all(b"yes\n").expect("Failed to write to stdin");

		// Write "local" to the second prompt
		stdin.write_all(b"local\n").expect("Failed to write to stdin");

		println!("Writing '\\n' (Enter) to stdin");
		// Press enter for the third prompt
		stdin.write_all(b"\n").expect("Failed to write to stdin");

		// Close stdin to indicate that no more input will be provided
		drop(stdin);

		let output = process
			.wait_with_output()
			.expect("Failed to read command output");

		if !output.stdout.is_empty() {
			println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
		}
	
		if !output.stderr.is_empty() {
			eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
		}

		let output = Command::new("movement")
			.args(&[
				"move", 
				"create-resource-account-and-publish-package",
				"--assume-yes",
				"--address-name",
				"moveth", 
				"--seed",
				"1234",
				"--package-dir", 
				"../move-modules"
			])
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.output()
			.expect("Failed to execute command");
	
		if !output.stdout.is_empty() {
			println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
		}
	
		if !output.stderr.is_empty() {
			eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
		}

		Ok(())
	}

	pub fn lock_bridge_transfer_assets(
		&self,
		//caller: &signer,
		initiator: Vec<u8>, //eth address
		bridge_transfer_id: Vec<u8>,
		hash_lock: Vec<u8>,
		time_lock: u64,
		recipient: Vec<u8>,
		amount: u64
	) {
		let output = Command::new("movement")
		.args(&[
			"move", 
			"create-resource-account-and-publish-package",
			"--assume-yes",
			"--address-name",
			"moveth", 
			"--seed",
			"1234",
			"--package-dir", 
			"../move-modules"
		])
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.output()
		.expect("Failed to execute command");

		if !output.stdout.is_empty() {
			println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
		}

		if !output.stderr.is_empty() {
			eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
		}

	}
	
	pub fn rest_client(&self) -> &Client {
		&self.rest_client
	}

	pub fn faucet_client(&self) -> Result<&Arc<RwLock<FaucetClient>>> {
		if let Some(faucet_client) = &self.faucet_client {
			Ok(faucet_client)
		} else {
			Err(anyhow::anyhow!("Faucet client not initialized"))
		}
	}
}

#[async_trait::async_trait]
impl BridgeContractCounterparty for MovementClient {
	type Address = MovementAddress;
	type Hash = [u8; 32];

	async fn lock_bridge_transfer_assets(
		&mut self,
		bridge_transfer_id: BridgeTransferId<Self::Hash>,
		hash_lock: HashLock<Self::Hash>,
		time_lock: TimeLock,
		initiator: InitiatorAddress<Vec<u8>>,
		recipient: RecipientAddress<Self::Address>,
		amount: Amount,
	) -> BridgeContractCounterpartyResult<()> {
		//@TODO properly return an error instead of unwrapping
		let args = vec![
			to_bcs_bytes(&initiator.0).unwrap(),
			to_bcs_bytes(&bridge_transfer_id.0).unwrap(),
			to_bcs_bytes(&hash_lock.0).unwrap(),
			to_bcs_bytes(&time_lock.0).unwrap(),
			to_bcs_bytes(&recipient.0).unwrap(),
			to_bcs_bytes(&amount.0).unwrap(),
		];
		let payload = utils::make_aptos_payload(
			self.counterparty_address,
			COUNTERPARTY_MODULE_NAME,
			"lock_bridge_transfer_assets",
			self.counterparty_type_args(Call::Lock),
			args,
		);
		let _ = utils::send_aptos_transaction(&self.rest_client, self.signer.as_ref(), payload)
			.await
			.map_err(|_| BridgeContractCounterpartyError::LockTransferAssetsError);
		Ok(())
	}

	async fn complete_bridge_transfer(
		&mut self,
		bridge_transfer_id: BridgeTransferId<Self::Hash>,
		preimage: HashLockPreImage,
	) -> BridgeContractCounterpartyResult<()> {
		let args = vec![
			to_bcs_bytes(&self.signer.address()).unwrap(),
			to_bcs_bytes(&bridge_transfer_id.0).unwrap(),
			to_bcs_bytes(&preimage.0).unwrap(),
		];
		let payload = utils::make_aptos_payload(
			self.counterparty_address,
			COUNTERPARTY_MODULE_NAME,
			"complete_bridge_transfer",
			self.counterparty_type_args(Call::Complete),
			args,
		);

		let _ = utils::send_aptos_transaction(&self.rest_client, self.signer.as_ref(), payload)
			.await
			.map_err(|_| BridgeContractCounterpartyError::CompleteTransferError);
		Ok(())
	}

	async fn abort_bridge_transfer(
		&mut self,
		bridge_transfer_id: BridgeTransferId<Self::Hash>,
	) -> BridgeContractCounterpartyResult<()> {
		let args = vec![
			to_bcs_bytes(&self.signer.address()).unwrap(),
			to_bcs_bytes(&bridge_transfer_id.0).unwrap(),
		];
		let payload = utils::make_aptos_payload(
			self.counterparty_address,
			COUNTERPARTY_MODULE_NAME,
			"abort_bridge_transfer",
			self.counterparty_type_args(Call::Abort),
			args,
		);
		let _ = utils::send_aptos_transaction(&self.rest_client, self.signer.as_ref(), payload)
			.await
			.map_err(|_| BridgeContractCounterpartyError::AbortTransferError);
		Ok(())
	}

	async fn get_bridge_transfer_details(
		&mut self,
		_bridge_transfer_id: BridgeTransferId<Self::Hash>,
	) -> BridgeContractCounterpartyResult<Option<BridgeTransferDetails<Self::Address, Self::Hash>>>
	{
		// let _ = utils::send_view_request(
		// 	self.rest_client,
		// 	self.counterparty_address,
		// 	"atomic_bridge_counterparty".to_string(),
		// );
		todo!();
	}
}

impl MovementClient {
	fn counterparty_type_args(&self, call: Call) -> Vec<TypeTag> {
		match call {
			Call::Lock => vec![TypeTag::Address, TypeTag::U64, TypeTag::U64, TypeTag::U8],
			Call::Complete => vec![TypeTag::Address, TypeTag::U64, TypeTag::U8],
			Call::Abort => vec![TypeTag::Address, TypeTag::U64],
			Call::GetDetails => vec![TypeTag::Address, TypeTag::U64],
		}
	}
}

fn to_bcs_bytes<T>(value: &T) -> Result<Vec<u8>, anyhow::Error>
where
	T: Serialize,
{
	Ok(bcs::to_bytes(value)?)
}
