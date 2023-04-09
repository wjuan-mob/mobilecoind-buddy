use crate::{Config, TokenId, TokenInfo, ConnectionUriGrpcioChannel};
//use rust_decimal::Decimal;
use displaydoc::Display;
use grpcio::ChannelBuilder;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread::{JoinHandle};
use std::time::Duration;
use mc_account_keys::{AccountKey};
use mc_mobilecoind_api::{self as api, mobilecoind_api_grpc::MobilecoindApiClient};
use mc_util_keyfile::read_keyfile;
use mc_api::{external, printable::PrintableWrapper};
use tracing::{event, span, Level};

/// The state and handle to the background worker, which owns the server connections.
/// This object exposes various getters to help the UI render the correct data without
/// blocking the UI thread, and allows for things like submitting a transaction.
pub struct Worker {
    /// Our startup parameters
    #[allow(unused)]
    config: Config,
    /// The connection to mobilecoind
    #[allow(unused)]
    mobilecoind_api_client: MobilecoindApiClient,
    /// The account key holding our funds
    #[allow(unused)]
    account_key: AccountKey,
    /// The monitor id we registered account with in mobilecoind
    monitor_id: Vec<u8>,
    /// The b58 public address of this account
    monitor_b58_address: String,
    /// The minimum fees for this network
    minimum_fees: HashMap<TokenId, u64>,
    /// The state that is mutable after initialization (updated by worker thread)
    state: Arc<Mutex<WorkerState>>,
    /// The worker thread handle
    join_handle: Option<JoinHandle<()>>,
    /// The stop requested flag to stop the worker
    stop_requested: Arc<AtomicBool>,
}

struct WorkerState {
    /// Synced blocks on this monitor id
    pub synced_blocks: u64,
    /// Total blocks in the ledger
    pub total_blocks: u64,
    /// The current balance of this account
    pub balance: HashMap<TokenId, u64>,
    /// A buffer of errors
    pub errors: VecDeque<String>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take() {
            self.stop_requested.store(true, Ordering::SeqCst);
            join_handle.join().expect("worker thread panicked");
        }
    }
}

impl Worker {
    pub fn new(config: Config) -> Result<Arc<Self>, WorkerInitError> {
        // Search for keyfile and load it
        let account_key = read_keyfile(config.keyfile.clone()).expect("Could not load keyfile");

        // Set up the gRPC connection to the mobilecoind client
        // Note: choice of 2 completion queues here is not very deliberate
        let grpc_env = Arc::new(grpcio::EnvBuilder::new().cq_count(2).build());
        let ch = ChannelBuilder::default_channel_builder(grpc_env)
            .connect_to_uri(&config.mobilecoind_uri);

        let mobilecoind_api_client = MobilecoindApiClient::new(ch);

        let mut retries = 10;
        let (monitor_id, _monitor_public_address, monitor_b58_address, minimum_fees) = loop {
            match Self::try_new_mobilecoind(&mobilecoind_api_client, &account_key) {
                Ok(result) => break result,
                Err(err) => event!(Level::ERROR, "Initialization failed, will retry: {}", err),
            }
            if retries == 0 {
                return Err(WorkerInitError::Mobilecoind);
            }
            retries -= 1;
            std::thread::sleep(Duration::from_millis(1000));
        };

        let state = Arc::new(Mutex::new(WorkerState {
                synced_blocks: 0,
                total_blocks: 1,
                balance: Default::default(),
                errors: Default::default(),                
            }));

        let stop_requested = Arc::new(AtomicBool::default());
        let thread_stop_requested = stop_requested.clone();
        let thread_monitor_id = monitor_id.clone();
        let thread_client = mobilecoind_api_client.clone();
        let thread_minimum_fees = minimum_fees.clone();
        let thread_state = state.clone();

        let join_handle = Some(std::thread::spawn(move || Self::worker_thread_entrypoint(
            thread_monitor_id,
            thread_client,
            thread_minimum_fees,
            thread_state,
            thread_stop_requested,
        )));

        Ok(Arc::new(Worker {
            config,
            mobilecoind_api_client,
            account_key,
            monitor_id,
            monitor_b58_address,
            minimum_fees,
            state,
            join_handle,
            stop_requested,
        }))
    }

    pub fn get_b58_address(&self) -> String {
        self.monitor_b58_address.clone()
    }

    pub fn get_sync_percent(&self) -> String {
        let st = self.state.lock().unwrap();
        let fraction = st.synced_blocks as f64 / st.total_blocks as f64;
        format!("{:.1}", fraction * 100f64)
    }

    pub fn get_token_info(&self) -> Vec<TokenInfo> {
        // Hard-coded symbol and decimals per token id
        let result = vec![
            TokenInfo {
                token_id: 0.into(),
                symbol: "MOB".to_string(),
                fee: 9999,
                decimals: 12,
            },
            TokenInfo {
                token_id: 1.into(),
                symbol: "EUSD".to_string(),
                fee: 9999,
                decimals: 6,
            },
            TokenInfo {
                token_id: 8192.into(),
                symbol: "FauxUSD".to_string(),
                fee: 9999,
                decimals: 6,
            },
        ];
        // Filter by which of these are actually defined on the given network
        result.into_iter().filter_map(|mut info|
            if let Some(fee) = self.minimum_fees.get(&info.token_id) {
                info.fee = *fee;
                Some(info)
            } else {
                None
            }).collect()
    }

    /// Get the balances of the monitored account.
    pub fn get_balances(&self) -> HashMap<TokenId, u64> {
        self.state.lock().unwrap().balance.clone()
    }

    /// Decode a b58 address
    pub fn decode_b58_address(b58_address: &str) -> Result<external::PublicAddress, String> {
           let printable_wrapper = PrintableWrapper::b58_decode(b58_address.to_owned())
                            .map_err(|err| format!("Invalid address: {err}"))?;

           if !printable_wrapper.has_public_address() {
                return Err("not a public address".to_string());
           };

           Ok(printable_wrapper.get_public_address().clone())    
    }

    /// Send money from the monitored account to the specified recipient
    pub fn send(&self, value: u64, token_id: TokenId, recipient: String) {
        span!(Level::INFO, "send payment");
        event!(Level::INFO, "send: {} of {} to {}", value, *token_id, recipient);

        let receiver = match Self::decode_b58_address(&recipient) {
            Ok(receiver) => receiver,
            Err(err) => {
                event!(Level::ERROR, "decoding b58: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err);
                return;
            }
        };

        let mut outlay = api::Outlay::new();
        outlay.value = value;
        outlay.set_receiver(receiver);

        let mut req = api::SendPaymentRequest::new();
        req.set_sender_monitor_id(self.monitor_id.clone());
        req.set_outlay_list(vec![outlay].into());
        req.token_id = *token_id;

        match self.mobilecoind_api_client.send_payment(&req) {
            Ok(_) => { event!(Level::INFO, "submitted payment successfully"); }
            Err(err) => {
                event!(Level::ERROR, "failed to submit payment: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
            }
        }
    }

    /// Get the error at the front of the error queue, if any.
    pub fn top_error(&self) -> Option<String> {
        self.state.lock().unwrap().errors.get(0).cloned()
    }

    /// Pop the error from the front of the error queue, if any.
    pub fn pop_error(&self) {
        self.state.lock().unwrap().errors.pop_front();
    }

    // Try to issue commands to mobilecoind to set up a new account, returning an
    // error if any of them fail
    //
    // Returns monitor id, monitor public address, monitor b58 address, and the
    // current network minimum fees
    fn try_new_mobilecoind(
        mobilecoind_api_client: &MobilecoindApiClient,
        account_key: &AccountKey,
    ) -> Result<
        (
            Vec<u8>,
            mc_api::external::PublicAddress,
            String,
            HashMap<TokenId, u64>,
        ),
        String,
    > {
        // Create a monitor using our account key
        let monitor_id = {
            let mut req = api::AddMonitorRequest::new();
            req.set_account_key(account_key.into());
            req.set_num_subaddresses(2);
            req.set_name("mobilecoind-buddy".to_string());

            let resp = mobilecoind_api_client
                .add_monitor(&req)
                .map_err(|err| format!("Failed adding a monitor: {err}"))?;

            resp.monitor_id
        };

        // Get the b58 public address for monitor
        let monitor_b58_address = {
            let mut req = api::GetPublicAddressRequest::new();
            req.set_monitor_id(monitor_id.clone());

            let resp = mobilecoind_api_client
                .get_public_address(&req)
                .map_err(|err| format!("Failed getting public address: {err}"))?;

            resp.b58_code
        };

        let monitor_printable_wrapper = PrintableWrapper::b58_decode(monitor_b58_address.clone())
            .expect("Could not decode b58 address");
        assert!(monitor_printable_wrapper.has_public_address());
        let monitor_public_address = monitor_printable_wrapper.get_public_address();

        // Get the network minimum fees and compute faucet amounts
        let minimum_fees = {
            let mut result = HashMap::<TokenId, u64>::default();

            let resp = mobilecoind_api_client
                .get_network_status(&Default::default())
                .map_err(|err| format!("Failed getting network status: {err}"))?;

            for (k, v) in resp.get_last_block_info().minimum_fees.iter() {
                result.insert(k.into(), *v);
            }

            result
        };

        Ok((
            monitor_id,
            monitor_public_address.clone(),
            monitor_b58_address,
            minimum_fees,
        ))
    }

    fn worker_thread_entrypoint(
            monitor_id: Vec<u8>,
            mobilecoind_api_client: MobilecoindApiClient,
            minimum_fees: HashMap<TokenId, u64>,
            state: Arc<Mutex<WorkerState>>,
            stop_requested: Arc<AtomicBool>,    
    ) {
        loop {
            if stop_requested.load(Ordering::SeqCst) {
                break;
            }

            if let Err(err) = Self::poll_mobilecoind(
                &monitor_id,
                &mobilecoind_api_client,
                &minimum_fees,
                &state
            ) {
                event!(Level::ERROR, "polling mobilecoind: {}", err);
                {
                    let mut st = state.lock().unwrap();
                    if st.errors.len() < 3 {
                        // TODO: Maybe pop an error instead?
                        st.errors.push_back(err.to_string());
                    }
                }
                // Back off for 500 ms when there is an error
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }


            // Back off for 100 ms
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn poll_mobilecoind(
        monitor_id: &Vec<u8>,
        client: &MobilecoindApiClient,
        minimum_fees: &HashMap<TokenId, u64>,
        state: &Arc<Mutex<WorkerState>>,
    ) -> Result<(), grpcio::Error> {
            // Check ledger status
            {
                let info = client.get_ledger_info(&Default::default())?;
                let mut st = state.lock().unwrap();
                st.total_blocks = info.block_count;                
            }

            // Check monitor status
            {
                let mut req = api::GetMonitorStatusRequest::new();
                req.set_monitor_id(monitor_id.clone());
                let resp = client.get_monitor_status(&req)?;

                let mut st = state.lock().unwrap();
                st.synced_blocks = resp.get_status().next_block;
            }

            // Get balance
            {
                for token_id in minimum_fees.keys() {
                    // FIXME: We should also check some other subaddresses most likely
                    let mut req = api::GetBalanceRequest::new();
                    req.set_monitor_id(monitor_id.clone());
                    req.set_token_id(**token_id);
                    let resp = client.get_balance(&req)?;

                    let mut st = state.lock().unwrap();
                    *st.balance.entry(*token_id).or_default() = resp.balance;
                }
            }
        Ok(())
    }
}

/// An error returned by the worker that prevented initialization.
/// Errors that occur after initalization are logged, and sent to the self.errors queue for display to the user.
#[derive(Clone, Debug, Display)]
pub enum WorkerInitError {
    /// Failed to initialize with mobilecoind
    Mobilecoind
}
