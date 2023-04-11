use crate::{Amount, Config, ConnectionUriGrpcioChannel, TokenId, TokenInfo, ValidatedQuote};
use deqs_api::{deqs as d_api, deqs_grpc::DeqsClientApiClient as DeqsClient};
use displaydoc::Display;
use grpcio::ChannelBuilder;
use mc_account_keys::AccountKey;
use mc_api::{external, printable::PrintableWrapper};
use mc_mobilecoind_api::{self as mcd_api, mobilecoind_api_grpc::MobilecoindApiClient, TxStatus};
use mc_transaction_extra::SignedContingentInput;
use mc_util_keyfile::read_keyfile;
use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{event, span, Level};

const QUOTES_LIMIT: u64 = 10;

/// The state and handle to the background worker, which owns the server connections.
/// This object exposes various getters to help the UI render the correct data without
/// blocking the UI thread, and allows for things like submitting a transaction.
pub struct Worker {
    /// Our startup parameters
    #[allow(unused)]
    config: Config,
    /// The connection to mobilecoind
    mobilecoind_api_client: MobilecoindApiClient,
    /// The connection to deqs (if any)
    deqs_client: Option<DeqsClient>,
    /// The account key holding our funds
    #[allow(unused)]
    account_key: AccountKey,
    /// The monitor id we registered account with in mobilecoind
    monitor_id: Vec<u8>,
    /// The proto public address of this account
    monitor_public_address: external::PublicAddress,
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

#[derive(Default)]
struct WorkerState {
    /// Synced blocks on this monitor id
    pub synced_blocks: u64,
    /// Total blocks in the ledger
    pub total_blocks: u64,
    /// The current balance of this account
    pub balance: HashMap<TokenId, u64>,
    /// The current token ids to poll for deqs
    /// Empty if the user is not trying to swap right now
    pub get_quotes_token_ids: Option<(TokenId, TokenId)>,
    /// The quotes we currently know about in the quote books
    pub quote_books: HashMap<(TokenId, TokenId), Vec<ValidatedQuote>>,
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
        let ch = ChannelBuilder::default_channel_builder(grpc_env.clone())
            .connect_to_uri(&config.mobilecoind_uri);

        let mobilecoind_api_client = MobilecoindApiClient::new(ch);

        let mut retries = 10;
        let (monitor_id, monitor_public_address, monitor_b58_address, minimum_fees) = loop {
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

        let deqs_client = config.deqs_uri.as_ref().map(|uri| {
            let ch = ChannelBuilder::default_channel_builder(grpc_env).connect_to_uri(uri);

            DeqsClient::new(ch)
        });

        let state = Arc::new(Mutex::new(WorkerState {
            total_blocks: 1,
            ..Default::default()
        }));

        let stop_requested = Arc::new(AtomicBool::default());
        let thread_stop_requested = stop_requested.clone();
        let thread_monitor_id = monitor_id.clone();
        let thread_mcd_client = mobilecoind_api_client.clone();
        let thread_deqs_client = deqs_client.clone();
        let thread_minimum_fees = minimum_fees.clone();
        let thread_state = state.clone();

        let join_handle = Some(std::thread::spawn(move || {
            Self::worker_thread_entrypoint(
                thread_monitor_id,
                thread_mcd_client,
                thread_deqs_client,
                thread_minimum_fees,
                thread_state,
                thread_stop_requested,
            )
        }));

        Ok(Arc::new(Worker {
            config,
            mobilecoind_api_client,
            deqs_client,
            account_key,
            monitor_id,
            monitor_public_address,
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

    pub fn get_sync_progress(&self) -> (u64, u64) {
        let st = self.state.lock().unwrap();
        (st.synced_blocks, st.total_blocks)
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
        result
            .into_iter()
            .filter_map(|mut info| {
                if let Some(fee) = self.minimum_fees.get(&info.token_id) {
                    info.fee = *fee;
                    Some(info)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get the balances of the monitored account.
    pub fn get_balances(&self) -> HashMap<TokenId, u64> {
        self.state.lock().unwrap().balance.clone()
    }

    /// Check if the worker has a deqs connection
    pub fn has_deqs(&self) -> bool {
        self.deqs_client.is_some()
    }

    /// Ask the worker to get quotes for given token ids
    pub fn get_quotes_for_token_ids(&self, tok1: TokenId, tok2: TokenId) {
        self.state.lock().unwrap().get_quotes_token_ids = Some((tok1, tok2));
    }

    /// Tell the worker it can stop getting quotes
    pub fn stop_quotes(&self) {
        self.state.lock().unwrap().get_quotes_token_ids = None;
    }

    /// Get the quote book for a given pair
    pub fn get_quote_book(&self, tok1: TokenId, tok2: TokenId) -> Vec<ValidatedQuote> {
        self.state
            .lock()
            .unwrap()
            .quote_books
            .get(&(tok1, tok2))
            .cloned()
            .unwrap_or(Default::default())
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
        event!(
            Level::INFO,
            "send: {} of {} to {}",
            value,
            *token_id,
            recipient
        );

        let receiver = match Self::decode_b58_address(&recipient) {
            Ok(receiver) => receiver,
            Err(err) => {
                event!(Level::ERROR, "decoding b58: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err);
                return;
            }
        };

        let mut outlay = mcd_api::Outlay::new();
        outlay.value = value;
        outlay.set_receiver(receiver);

        let mut req = mcd_api::SendPaymentRequest::new();
        req.set_sender_monitor_id(self.monitor_id.clone());
        req.set_outlay_list(vec![outlay].into());
        req.token_id = *token_id;

        match self.mobilecoind_api_client.send_payment(&req) {
            Ok(_) => {
                event!(Level::INFO, "submitted payment successfully");
            }
            Err(err) => {
                event!(Level::ERROR, "failed to submit payment: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
            }
        }
    }

    /// Create and submit a swap offer
    pub fn offer_swap(&self, from_amount: Amount, to_amount: Amount) {
        span!(Level::INFO, "offer_swap");
        // FIXME: There should not be any unwraps, we should split this out into a helper function probably
        let selected_utxo = match self.get_specific_utxo(from_amount) {
            Ok(utxo) => utxo,
            Err(err) => {
                event!(
                    Level::ERROR,
                    "failed to obtain required utxo for swap: {}",
                    err
                );
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
                return;
            }
        };

        // Ask mobilecoind to sign an SCI over this input
        let mut request = mcd_api::GenerateSwapRequest::new();
        request.set_sender_monitor_id(self.monitor_id.clone());
        request.set_change_subaddress(0);
        request.set_input(selected_utxo);
        request.set_allow_partial_fill(true);
        request.set_counter_value(to_amount.value);
        request.set_counter_token_id(*to_amount.token_id);
        // Arbitrarily, minimum fill value is 10 * minimum_fee
        let min_fill_value = self
            .minimum_fees
            .get(&from_amount.token_id)
            .cloned()
            .unwrap_or(0)
            * 10;
        request.set_minimum_fill_value(min_fill_value);
        let mut response = match self.mobilecoind_api_client.generate_swap(&request) {
            Ok(resp) => resp,
            Err(err) => {
                event!(Level::ERROR, "mobilecoind generate_swap rpc: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
                return;
            }
        };

        let proto_sci = response.take_sci();

        let sci = match SignedContingentInput::try_from(&proto_sci) {
            Ok(sci) => sci,
            Err(err) => {
                event!(
                    Level::ERROR,
                    "mobilecoind generated a malformed sci: {}",
                    err
                );
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
                return;
            }
        };

        if let Err(err) = sci.validate() {
            event!(
                Level::ERROR,
                "mobilecoind generated an invalid sci: {}",
                err
            );
            let mut st = self.state.lock().unwrap();
            st.errors.push_back(err.to_string());
            return;
        };

        // Submit the generated sci to the deqs
        let mut request = d_api::SubmitQuotesRequest::new();
        request.set_quotes(vec![proto_sci].into());
        let response = match self.deqs_client.as_ref().unwrap().submit_quotes(&request) {
            Ok(resp) => resp,
            Err(err) => {
                event!(Level::ERROR, "deqs submit_quotes rpc: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
                return;
            }
        };
        // Handle any error statuses and error messages
        if response.status_codes.len() > 1 {
            event!(
                Level::WARN,
                "unexpectedly got {} status codes back",
                response.status_codes.len()
            );
        }
        let status_code = response.status_codes.get(0);
        if status_code == Some(&d_api::QuoteStatusCode::CREATED) {
            event!(Level::INFO, "submitted swap offer successfully");
        } else {
            let err_msg = response
                .error_messages
                .get(0)
                .cloned()
                .unwrap_or("no error message...".to_owned());
            event!(
                Level::ERROR,
                "deqs error: {:?}: {}",
                status_code
                    .map(|c| format!("{:?}", c))
                    .unwrap_or("no status".to_owned()),
                err_msg
            );
            let mut st = self.state.lock().unwrap();
            st.errors.push_back(err_msg);
        }
    }

    // Helper for offer_swap.
    //
    // Tries to construct a utxo with a specific value
    fn get_specific_utxo(&self, from_amount: Amount) -> Result<mcd_api::UnspentTxOut, String> {
        // Allow at most 5 errors
        let mut retries = 5;
        loop {
            let mut request = mcd_api::GetUnspentTxOutListRequest::new();
            request.set_monitor_id(self.monitor_id.clone());
            request.set_subaddress_index(0);
            request.set_token_id(*from_amount.token_id);
            let response = match self
                .mobilecoind_api_client
                .get_unspent_tx_out_list(&request)
            {
                Ok(resp) => resp,
                Err(err) => {
                    let err_msg = format!("failed getting unspent tx out list: {err}");
                    event!(Level::ERROR, err_msg);
                    retries -= 1;
                    if retries == 0 {
                        return Err(err_msg);
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };

            if let Some(utxo) = response.output_list.iter().find(|utxo| {
                utxo.token_id == *from_amount.token_id && utxo.value == from_amount.value
            }) {
                return Ok(utxo.clone());
            }
            retries -= 1;
            if retries == 0 {
                let err_msg = "failed to produce input of required value".to_owned();
                event!(Level::ERROR, err_msg);
                return Err(err_msg);
            }
            // Produce a self-payment in this amount, then wait for it to land
            span!(Level::INFO, "self payment");
            event!(Level::INFO, "attempting self payment before swap offer");
            let mut outlay = mcd_api::Outlay::new();
            outlay.set_value(from_amount.value);
            outlay.set_receiver(self.monitor_public_address.clone());
            let mut request = mcd_api::SendPaymentRequest::new();
            request.set_sender_monitor_id(self.monitor_id.clone());
            request.set_sender_subaddress(0);
            request.set_token_id(*from_amount.token_id);
            request.set_outlay_list(vec![outlay].into());
            let mut response = match self.mobilecoind_api_client.send_payment(&request) {
                Ok(resp) => resp,
                Err(err) => {
                    let err_msg = format!("failed submitting self-payment: {err}");
                    event!(Level::ERROR, err_msg);
                    retries -= 1;
                    if retries == 0 {
                        return Err(err_msg);
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };

            // Coerce this into a SubmitTxResponse, so that we can use it with get_tx_status_as_sender
            let mut submit_tx_response = mcd_api::SubmitTxResponse::new();
            submit_tx_response.set_sender_tx_receipt(response.take_sender_tx_receipt());
            submit_tx_response
                .set_receiver_tx_receipt_list(response.take_receiver_tx_receipt_list());

            // Wait for self payment to land
            loop {
                let resp = match self
                    .mobilecoind_api_client
                    .get_tx_status_as_sender(&submit_tx_response)
                {
                    Ok(resp) => resp,
                    Err(err) => {
                        event!(Level::ERROR, "get tx status: {}", err);
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                };
                std::thread::sleep(Duration::from_millis(50));
                if resp.status != TxStatus::Unknown && resp.status != TxStatus::Verified {
                    event!(
                        Level::WARN,
                        "got a strange status from self payment Tx: {:?}",
                        resp.status
                    );
                }
                if resp.status != TxStatus::Unknown {
                    break;
                }
            }
            // Extra sleep, try to give the sync thread time to find the utxo
            // FIXME: Should we block on a different call than get_tx_status?
            // Or maybe keep track of the expected utxo and retry on get_unspent_utxos until we find it?
            std::thread::sleep(Duration::from_millis(1000));
        }
    }

    /// Act as the counterparty to a given swap
    ///
    /// Arguments:
    /// sci - sci to fulfill
    /// partial_fill_value - degree to fill it to
    /// from_token_id - the token id we need to pay in order to fulfill the sci
    /// fee_token_id - the token id to pay the fee in
    pub fn perform_swap(
        &self,
        sci: SignedContingentInput,
        partial_fill_value: u64,
        from_token_id: TokenId,
        fee_token_id: TokenId,
    ) {
        // First we have to get utxo list from mobilecoind
        let mut retries = 3;
        let mut response = loop {
            let mut request = mcd_api::GetUnspentTxOutListRequest::new();
            request.set_monitor_id(self.monitor_id.clone());
            request.set_subaddress_index(0);
            request.set_token_id(*from_token_id);
            match self
                .mobilecoind_api_client
                .get_unspent_tx_out_list(&request)
            {
                Ok(resp) => break resp,
                Err(err) => {
                    let err_msg = format!("failed getting unspent tx out list: {err}");
                    event!(Level::ERROR, err_msg);
                    retries -= 1;
                    if retries == 0 {
                        let mut st = self.state.lock().unwrap();
                        st.errors.push_back(err_msg);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            };
        };

        let mut sci_for_tx = mcd_api::SciForTx::new();
        sci_for_tx.set_sci((&sci).into());
        sci_for_tx.set_partial_fill_value(partial_fill_value);

        let mut req = mcd_api::GenerateMixedTxRequest::new();
        req.set_sender_monitor_id(self.monitor_id.clone());
        req.set_change_subaddress(0);
        req.set_input_list(response.take_output_list());
        req.set_scis(vec![sci_for_tx].into());
        req.set_fee_token_id(*fee_token_id);

        let mut resp = match self.mobilecoind_api_client.generate_mixed_tx(&req) {
            Ok(resp) => {
                event!(Level::DEBUG, "generated swap tx successfully");
                resp
            }
            Err(err) => {
                event!(Level::ERROR, "failed to generate swap tx: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
                return;
            }
        };

        let mut req = mcd_api::SubmitTxRequest::new();
        req.set_tx_proposal(resp.take_tx_proposal());

        match self.mobilecoind_api_client.submit_tx(&req) {
            Ok(_resp) => {
                event!(Level::INFO, "submitted swap tx successfully");
            }
            Err(err) => {
                event!(Level::ERROR, "failed to submit swap tx: {}", err);
                let mut st = self.state.lock().unwrap();
                st.errors.push_back(err.to_string());
                return;
            }
        };
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
            let mut req = mcd_api::AddMonitorRequest::new();
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
            let mut req = mcd_api::GetPublicAddressRequest::new();
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
        deqs_client: Option<DeqsClient>,
        minimum_fees: HashMap<TokenId, u64>,
        state: Arc<Mutex<WorkerState>>,
        stop_requested: Arc<AtomicBool>,
    ) {
        loop {
            if stop_requested.load(Ordering::SeqCst) {
                break;
            }

            event!(Level::TRACE, "worker: polling loop");

            if let Err(err) =
                Self::poll_mobilecoind(&monitor_id, &mobilecoind_api_client, &minimum_fees, &state)
            {
                event!(Level::ERROR, "polling mobilecoind: {}", err);
                {
                    let mut st = state.lock().unwrap();
                    // TODO: Maybe pop an error if there are many errors?
                    if st.errors.len() < 3 {
                        st.errors.push_back(err.to_string());
                    }
                }
                // Back off for 500 ms when there is an error
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }

            if let Some(deqs_client) = deqs_client.as_ref() {
                if let Err(err) = Self::poll_deqs(deqs_client, &state) {
                    event!(Level::ERROR, "polling deqs: {}", err);
                    {
                        let mut st = state.lock().unwrap();
                        // TODO: Maybe pop an error if there are many errors?
                        if st.errors.len() < 3 {
                            st.errors.push_back(err.to_string());
                        }
                    }
                    // Back off for 500 ms when there is an error
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }

            // Back off for 20 ms
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn poll_mobilecoind(
        monitor_id: &Vec<u8>,
        client: &MobilecoindApiClient,
        minimum_fees: &HashMap<TokenId, u64>,
        state: &Arc<Mutex<WorkerState>>,
    ) -> Result<(), grpcio::Error> {
        span!(Level::TRACE, "poll mobilecoind");
        // Check ledger status
        {
            event!(Level::TRACE, "worker: check ledger status");
            let info = client.get_ledger_info(&Default::default())?;
            let mut st = state.lock().unwrap();
            st.total_blocks = info.block_count;
        }

        // Check monitor status
        {
            event!(Level::TRACE, "worker: check monitor status");
            let mut req = mcd_api::GetMonitorStatusRequest::new();
            req.set_monitor_id(monitor_id.clone());
            let resp = client.get_monitor_status(&req)?;

            let mut st = state.lock().unwrap();
            st.synced_blocks = resp.get_status().next_block;
        }

        // Get balance
        {
            for token_id in minimum_fees.keys() {
                event!(Level::TRACE, "worker: check balance: {}", *token_id);
                // FIXME: We should also check some other subaddresses most likely
                let mut req = mcd_api::GetBalanceRequest::new();
                req.set_monitor_id(monitor_id.clone());
                req.set_token_id(**token_id);
                let resp = client.get_balance(&req)?;

                let mut st = state.lock().unwrap();
                *st.balance.entry(*token_id).or_default() = resp.balance;
            }
        }
        Ok(())
    }

    fn poll_deqs(
        client: &DeqsClient,
        state: &Arc<Mutex<WorkerState>>,
    ) -> Result<(), grpcio::Error> {
        let maybe_tokens = { state.lock().unwrap().get_quotes_token_ids.clone() };
        // Only do the poll if the ui thread told us we're looking at two particular tokens,
        // and then only if they are different tokens.
        if let Some((token1, token2)) = maybe_tokens {
            if token1 == token2 {
                return Ok(());
            }
            span!(Level::TRACE, "poll deqs");

            for (base_token_id, counter_token_id) in
                vec![(token1, token2), (token2, token1)].into_iter()
            {
                let mut pair = d_api::Pair::new();
                pair.set_base_token_id(*base_token_id);
                pair.set_counter_token_id(*counter_token_id);

                let mut req = d_api::GetQuotesRequest::new();
                req.set_pair(pair);
                req.set_limit(QUOTES_LIMIT);

                event!(
                    Level::TRACE,
                    "getting quotes for pair {} / {}",
                    *base_token_id,
                    *counter_token_id
                );
                let resp = client.get_quotes(&req)?;
                let validated_quotes: Vec<ValidatedQuote> = resp
                    .get_quotes()
                    .iter()
                    .filter_map(|quote| match ValidatedQuote::try_from(quote) {
                        Ok(validated_quote) => Some(validated_quote),
                        Err(err) => {
                            event!(Level::ERROR, "validating quote: {}", err);
                            None
                        }
                    })
                    .collect();
                {
                    let mut st = state.lock().unwrap();
                    *st.quote_books
                        .entry((base_token_id, counter_token_id))
                        .or_default() = validated_quotes;
                }
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
    Mobilecoind,
}
