// Copyright 2019-2022 Manta Network.
// This file is part of manta-signer.
//
// manta-signer is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// manta-signer is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with manta-signer. If not, see <http://www.gnu.org/licenses/>.

//! Manta Signer Service Implementation

use crate::{
    config::{Config, DataExistenceResponse, Setup},
    http,
    log::{info, trace, warn},
    secret::{Argon2, Authorizer, ExposeSecret, PasswordHash, SecretString},
};
use alloc::sync::Arc;
use core::{
    fmt::{self, Display},
    time::Duration,
};
use http_types::headers::HeaderValue;
use manta_accounting::{
    asset::Asset,
    fs::{cocoon::File, File as _, SaveError},
    key::AccountTable,
    transfer::canonical::TransferShape,
    wallet::signer::functions::default_authorization_context,
};
use manta_pay::{
    config::{Config as MantaPayConfig, Transaction},
    key::{Mnemonic, TestnetKeySecret},
    signer::{
        base::{Signer, SignerParameters, SignerState, UtxoAccumulator},
        client::network::{Message, Network, NetworkSpecific},
        AssetMetadata, TokenType,
    },
};

/// previous version of manta-pay state. Should use versioning system for SignerState
use previous_state_manta_pay::signer::base::SignerState as OldSignerState;

use manta_util::{from_variant, serde::Serialize};
use parking_lot::Mutex;
use std::{
    io,
    net::{AddrParseError, SocketAddr},
    path::Path,
};
use tide::{
    security::{CorsMiddleware, Origin},
    StatusCode,
};
use tokio::{
    fs,
    sync::Mutex as AsyncMutex,
    task::{self, JoinError},
};

pub use manta_pay::{
    config::{address_to_base58, Address},
    signer::{self, SignError, SignResponse, SyncError, SyncResponse},
};

/// Synchronization Request
pub type SyncRequest = Message<signer::SyncRequest>;

/// Signing Request
pub type SignRequest = Message<signer::SignRequest>;

/// Receiving Key Request
pub type ReceivingKeyRequest = Message<signer::GetRequest>;

/// Password Retry Interval
pub const PASSWORD_RETRY_INTERVAL: Duration = Duration::from_millis(1000);

/// Sets the task to sleep to delay password retry.
#[inline]
pub async fn delay_password_retry() {
    tokio::time::sleep(PASSWORD_RETRY_INTERVAL).await;
}

/// Service Error
#[derive(Debug)]
pub enum Error {
    /// Address Parsing Error
    AddrParseError(AddrParseError),

    /// Runtime Join Error
    JoinError(JoinError),

    /// Failed to Load SDK Parameters
    ParameterLoadingError,

    /// Save Error
    SaveError(SaveError<File>),

    /// Generic I/O Error
    Io(io::Error),

    /// Authorization Error
    AuthorizationError,

    /// Signer Delay Error
    ///
    /// The signer could not process the request at this time.
    Delayed,
}

from_variant!(Error, AddrParseError, AddrParseError);
from_variant!(Error, JoinError, JoinError);
from_variant!(Error, SaveError, SaveError<File>);
from_variant!(Error, Io, io::Error);

impl From<Error> for tide::Error {
    #[inline]
    fn from(err: Error) -> tide::Error {
        match err {
            Error::AuthorizationError => {
                Self::from_str(StatusCode::Unauthorized, "request was not authorized")
            }
            Error::Delayed => Self::from_str(
                StatusCode::Accepted,
                "another process is currently signing and this request should be tried again later",
            ),
            _ => Self::from_str(
                StatusCode::InternalServerError,
                "unable to complete request",
            ),
        }
    }
}

impl Display for Error {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::AddrParseError(err) => write!(f, "Address Parse Error: {err}"),
            Self::JoinError(err) => write!(f, "Join Error: {err}"),
            Self::ParameterLoadingError => write!(f, "Parameter Loading Error"),
            Self::SaveError(err) => write!(f, "Save Error: {err}"),
            Self::Io(err) => write!(f, "I/O Error: {err}"),
            Self::AuthorizationError => write!(f, "Authorization Error"),
            Self::Delayed => write!(f, "Delay Error"),
        }
    }
}

/// Result Type
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Returns a summary of the current transaction.
#[inline]
pub fn display_transaction(
    transaction: &Transaction,
    metadata: &AssetMetadata,
    network: Network,
) -> String {
    match transaction {
        Transaction::ToPrivate(Asset { value, .. }) => match metadata.token_type {
            TokenType::FT(decimals) => {
                format!(
                    "Privatize {} on {} network",
                    metadata.display(*value, decimals),
                    network
                )
            }
            TokenType::NFT => {
                format!("Privatize NFT on {} network", network)
            }
        },
        Transaction::PrivateTransfer(Asset { value, .. }, receiving_key) => {
            match metadata.token_type {
                TokenType::FT(decimals) => {
                    format!(
                        "Send {} to {} on {} network",
                        metadata.display(*value, decimals),
                        address_to_base58(receiving_key),
                        network
                    )
                }
                TokenType::NFT => {
                    format!(
                        "Send NFT to {} on {} network",
                        address_to_base58(receiving_key),
                        network
                    )
                }
            }
        }
        Transaction::ToPublic(Asset { value, .. }, _) => match metadata.token_type {
            TokenType::FT(decimals) => {
                format!(
                    "Public {} on {} network",
                    metadata.display(*value, decimals),
                    network
                )
            }
            TokenType::NFT => {
                format!("Public NFT on {} network", network)
            }
        },
    }
}

/// Checked Authorizer
struct CheckedAuthorizer<A>
where
    A: Authorizer,
{
    /// Password Hash
    password_hash: PasswordHash<Argon2>,

    /// Authorizer
    authorizer: A,
}

impl<A> CheckedAuthorizer<A>
where
    A: Authorizer,
{
    /// Checks that the authorizer's password matches the known password by sending the `prompt`.
    #[inline]
    async fn check<T>(&mut self, prompt: &T) -> Result<()>
    where
        T: Serialize,
    {
        self.authorizer.wake(prompt).await;
        loop {
            if let Some(password) = self.authorizer.password().await.known() {
                if self
                    .password_hash
                    .verify(password.expose_secret().as_bytes())
                    .is_ok()
                {
                    self.authorizer.sleep().await;
                    println!("Password is ok");
                    return Ok(());
                }
            } else {
                println!("Password is now known, returning auth error");
                return Err(Error::AuthorizationError);
            }
            delay_password_retry().await;
        }
    }
}

/// State
struct State {
    /// Configuration
    config: Config,

    /// Signer
    signer: NetworkSpecific<Signer>,

    /// Signing Flag
    currently_signing: bool,
}

/// Signer Server
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = ""))]
pub struct Server<A>
where
    A: Authorizer,
{
    /// Server State
    state: Arc<Mutex<State>>,

    /// Authorizer
    authorizer: Arc<AsyncMutex<CheckedAuthorizer<A>>>,
}

impl<A> Server<A>
where
    A: Authorizer,
{
    /// Builds a new [`Server`] from `config` and `authorizer`.
    #[inline]
    pub async fn build(config: Config, mut authorizer: A) -> Result<Self> {
        info!("building signer server")?;
        info!("loading latest parameters from Manta Parameters")?;
        let data_path = config.data_directory().to_owned();
        let parameters = task::spawn_blocking(move || crate::parameters::load(data_path))
            .await?
            .ok_or(Error::ParameterLoadingError)?;
        info!("setting up configuration")?;
        let backup_exists = config
            .check_all_backups()
            .await
            .expect("Unable to check for the existence of one more more backup files");
        if backup_exists {
            info!("backup file found, restoring backup.")?;
        }
        let data_exists = config.does_data_exist().await;
        let does_all_data_exist = data_exists.dolphin && data_exists.calamari && data_exists.manta;
        let does_one_data_exist = data_exists.dolphin || data_exists.calamari || data_exists.manta;
        let setup = authorizer.setup(does_one_data_exist).await;
        let (password_hash, dolphin_signer, calamari_signer, manta_signer) = match setup {
            Setup::CreateAccount(mnemonic) => loop {
                if let Some((_password, password_hash)) = Self::load_password(&mut authorizer).await
                {
                    info!("creating dolphin state.")?;
                    let dolphin_state = Self::create_state(
                        &config.data_path.dolphin,
                        &password_hash,
                        mnemonic.clone(),
                        &parameters,
                    )
                    .await?;

                    info!("creating calamari state.")?;
                    let calamari_state = Self::create_state(
                        &config.data_path.calamari,
                        &password_hash,
                        mnemonic.clone(),
                        &parameters,
                    )
                    .await?;

                    info!("creating manta state.")?;
                    let manta_state = Self::create_state(
                        &config.data_path.manta,
                        &password_hash,
                        mnemonic.clone(),
                        &parameters,
                    )
                    .await?;
                    break (
                        password_hash,
                        Signer::from_parts(parameters.clone(), dolphin_state),
                        Signer::from_parts(parameters.clone(), calamari_state),
                        Signer::from_parts(parameters.clone(), manta_state),
                    );
                }
                delay_password_retry().await;
            },
            Setup::Login => loop {
                if let Some((_, password_hash)) = Self::load_password(&mut authorizer).await {
                    let mut recovery_mnemonic: Option<Mnemonic> = None;
                    if !does_all_data_exist {
                        // Edge case: 1 or 2 out of 3 storage.dat files are missing.
                        // We need to get the mnemonic from one of the existing files
                        // and use it to recreate the missing files.
                        info!("not all data files were found, starting re-creation process from existing file's seed phrase.")?;
                        recovery_mnemonic = Self::get_mnemonic_for_missing_file(
                            data_exists,
                            &config,
                            &password_hash,
                            parameters.clone(),
                        )
                        .await;
                    }

                    let dolphin_state = Self::create_or_load_state(
                        !data_exists.dolphin,
                        &config.data_path.dolphin,
                        &password_hash,
                        recovery_mnemonic.clone(),
                        &parameters,
                    )
                    .await?;

                    let calamari_state = Self::create_or_load_state(
                        !data_exists.calamari,
                        &config.data_path.calamari,
                        &password_hash,
                        recovery_mnemonic.clone(),
                        &parameters,
                    )
                    .await?;

                    let manta_state = Self::create_or_load_state(
                        !data_exists.manta,
                        &config.data_path.manta,
                        &password_hash,
                        recovery_mnemonic.clone(),
                        &parameters,
                    )
                    .await?;

                    if let (Some(dolphin_state), Some(calamari_state), Some(manta_state)) =
                        (dolphin_state, calamari_state, manta_state)
                    {
                        break (
                            password_hash,
                            Signer::from_parts(parameters.clone(), dolphin_state),
                            Signer::from_parts(parameters.clone(), calamari_state),
                            Signer::from_parts(parameters.clone(), manta_state),
                        );
                    }
                }
                delay_password_retry().await;
            },
        };
        info!("telling authorizer to sleep")?;
        authorizer.sleep().await;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                config,
                signer: NetworkSpecific {
                    dolphin: dolphin_signer,
                    calamari: calamari_signer,
                    manta: manta_signer,
                },
                currently_signing: false,
            })),
            authorizer: Arc::new(AsyncMutex::new(CheckedAuthorizer {
                password_hash,
                authorizer,
            })),
        })
    }

    /// If one of the network storage files is missing, we need to load an existing network
    /// state and use it's recovery phrase to re-create the missing file. This function
    /// returns the seed phrase from an existing state file.
    #[inline]
    async fn get_mnemonic_for_missing_file(
        data_exists: DataExistenceResponse,
        config: &Config,
        password_hash: &PasswordHash<Argon2>,
        parameters: SignerParameters,
    ) -> Option<Mnemonic> {
        let existing_state_path = if data_exists.dolphin {
            &config.data_path.dolphin
        } else if data_exists.calamari {
            &config.data_path.calamari
        } else {
            &config.data_path.manta
        };
        let existing_signer = Signer::from_parts(
            parameters.clone(),
            Self::load_state(existing_state_path, password_hash, &parameters)
                .await
                .expect("Unable to get dolphin state")?,
        );
        Some(
            existing_signer
                .state()
                .accounts()
                .as_ref()
                .expect("No accounts in signer state!")
                .keys()
                .expose_mnemonic()
                .clone(),
        )
    }

    /// Recreates a new state from a mnemonic of an existing state
    /// if file was deleted or loads an existing state during login.
    #[inline]
    async fn create_or_load_state(
        should_recreate: bool,
        data_path: &Path,
        password_hash: &PasswordHash<Argon2>,
        recovery_mnemonic: Option<Mnemonic>,
        parameters: &SignerParameters,
    ) -> Result<Option<SignerState>> {
        if should_recreate {
            info!("state missing! recreating state.")?;
            let state = Self::create_state(
                data_path,
                password_hash,
                recovery_mnemonic.expect("unable to retrieve mnemonic for account recreation."),
                parameters,
            )
            .await
            .expect("Unable to recreate signer instance from existing mnemonic.");
            Ok(Some(state))
        } else {
            Self::load_state(data_path, password_hash, parameters).await
        }
    }

    /// If users cancels a transaction, this method will be called by front-end to indicate that
    /// the signer can now start signing new transactions.
    #[inline]
    pub async fn cancel_signing(&mut self) {
        self.state.lock().currently_signing = false;
        // Forcefully sleep because the authorizer gets stuck awake if we exit recovery window
        self.authorizer.lock().await.authorizer.sleep();
    }

    /// Starts the signer server with `config` and `authorizer`.
    #[inline]
    pub async fn start(self) -> Result<()> {
        let config = self.state.lock().config.clone();
        info!("performing service setup with {:#?}", config)?;
        let socket_address = config.service_url.parse::<SocketAddr>()?;
        let cors = CorsMiddleware::new()
            .allow_methods("GET, POST".parse::<HeaderValue>().unwrap())
            .allow_origin(if config.origin_urls.is_empty() {
                Origin::Any
            } else {
                Origin::List(config.origin_urls)
            })
            .allow_credentials(false);
        let mut api = tide::Server::with_state(self);
        api.with(cors);
        api.at("/version")
            .get(|_| http::into_body(Server::<A>::version));
        http::register_post(&mut api, "/sync", Server::sync);
        http::register_post(&mut api, "/sign", Server::sign);
        http::register_post(&mut api, "/address", Server::address);
        info!("serving signer API at {}", socket_address)?;
        api.listen(socket_address).await?;
        Ok(())
    }

    /// Loads the password from the `authorizer` and compute the password hash.
    #[inline]
    async fn load_password(authorizer: &mut A) -> Option<(SecretString, PasswordHash<Argon2>)> {
        info!("loading password from authorizer").ok()?;
        let password = authorizer.password().await.known()?;
        let password_hash = PasswordHash::from_default(password.expose_secret().as_bytes());
        Some((password, password_hash))
    }

    /// Creates the initial signer state for a new account.
    #[inline]
    async fn create_state(
        data_path: &Path,
        password_hash: &PasswordHash<Argon2>,
        mnemonic: Mnemonic,
        parameters: &SignerParameters,
    ) -> Result<SignerState> {
        info!("creating signer state")?;
        let mut state = SignerState::new(UtxoAccumulator::new(
            task::spawn_blocking(crate::parameters::load_utxo_accumulator_model)
                .await?
                .ok_or(Error::ParameterLoadingError)?,
        ));
        let accounts = AccountTable::new(TestnetKeySecret::new(mnemonic, ""));
        state.load_authorization_context(default_authorization_context::<MantaPayConfig>(
            &accounts,
            &parameters.parameters,
        ));
        state.load_accounts(accounts);

        info!("saving signer state")?;
        let data_path = data_path.to_owned();
        let password_hash_bytes = password_hash.as_bytes();
        let cloned_state = state.clone();
        task::spawn_blocking(move || File::save(&data_path, &password_hash_bytes, cloned_state))
            .await??;
        Ok(state)
    }

    /// Loads the signer state from the data path.
    #[inline]
    async fn load_state(
        data_path: &Path,
        password_hash: &PasswordHash<Argon2>,
        parameters: &SignerParameters,
    ) -> Result<Option<SignerState>> {
        info!("loading signer state from disk")?;
        let data_path_buf = data_path.to_owned();
        let password_hash_bytes = password_hash.as_bytes();

        let state_result = task::spawn_blocking(move || {
            File::load::<_, SignerState>(&data_path_buf, &password_hash_bytes)
        })
        .await?;

        if let Ok(correct_state) = state_result {
            Ok(Some(correct_state))
        } else {
            // fallback to try from old state version
            Self::new_state_from_old_state(data_path, password_hash, parameters).await
        }
    }

    /// Attempts to create new signer state from the old signer state version
    #[inline]
    async fn new_state_from_old_state(
        data_path: &Path,
        password_hash: &PasswordHash<Argon2>,
        parameters: &SignerParameters,
    ) -> Result<Option<SignerState>> {
        info!("loading mnemonic from old state")?;
        let data_path_buf = data_path.to_owned();
        let password_hash_bytes = password_hash.as_bytes();

        if let Ok(state) = task::spawn_blocking(move || {
            File::load::<_, OldSignerState>(&data_path_buf, &password_hash_bytes)
        })
        .await?
        {
            let mnemonic = state.accounts().keys().expose_mnemonic().clone();

            let encoded: Vec<u8> = bincode::serialize(&mnemonic).expect("encoding mnenomic failed");
            let new_mnemonic: Mnemonic =
                bincode::deserialize(&encoded[..]).expect("decoding mnenomic failed");

            let new_state =
                Self::create_state(data_path, password_hash, new_mnemonic, parameters).await?;

            Ok(Some(new_state))
        } else {
            Ok(None)
        }
    }

    /// Saves the signer state corresponding to `network` to disk.
    #[inline]
    async fn save(self, network: Network) -> Result<()> {
        info!("starting signer state save to disk for {}", network)?;
        let path = self.state.lock().config.data_path[network].clone();
        let backup = self.state.lock().config.backup_data_path[network].clone();
        fs::rename(&path, &backup).await?;
        let password_hash_bytes = self.authorizer.lock().await.password_hash.as_bytes();
        task::spawn_blocking(move || {
            let lock = self.state.lock();
            let state_to_save = lock.signer[network].state();
            File::save(path, &password_hash_bytes, state_to_save)
        })
        .await??;
        fs::remove_file(backup).await?;
        info!("save complete for {}", network)?;
        Ok(())
    }

    /// Returns the [`crate::VERSION`] string to the client.
    #[inline]
    pub async fn version() -> Result<&'static str> {
        trace!("[PING] current signer version: {}", crate::VERSION)?;
        Ok(crate::VERSION)
    }

    /// Runs the synchronization protocol on the signer.
    #[inline]
    pub async fn sync(self, request: SyncRequest) -> Result<Result<SyncResponse, SyncError>> {
        info!("[REQUEST] processing `sync`:  {:?}.", request)?;
        let response = self.state.lock().signer[request.network].sync(request.message);
        task::spawn(async move {
            if self.save(request.network).await.is_err() {
                let _ = warn!("unable to save current signer state");
            }
        });
        info!("[RESPONSE] responding to `sync` with: {:?}.", response)?;
        Ok(response)
    }

    /// Runs the transaction signing protocol on the signer.
    #[inline]
    pub async fn sign(self, request: SignRequest) -> Result<Result<SignResponse, SignError>> {
        info!("[REQUEST] processing `sign`: {:?}.", request)?;
        if self.state.lock().currently_signing {
            return Err(Error::Delayed);
        }
        self.state.lock().currently_signing = true;
        let SignRequest {
            network,
            message:
                signer::SignRequest {
                    transaction,
                    metadata,
                },
        } = request;
        match transaction.shape() {
            TransferShape::ToPrivate => {
                // NOTE: We skip authorization on mint transactions because they are deposits not
                //       withdrawals from the point of view of the signer. Everything else, by
                //       default, requests authorization.
            }
            _ => {
                info!("[AUTH] asking for transaction authorization")?;
                let summary = metadata
                    .map(|metadata| display_transaction(&transaction, &metadata, network))
                    .unwrap_or_default();
                self.authorizer.lock().await.check(&summary).await?
            }
        }
        let response = self.state.lock().signer[network].sign(transaction);
        info!("[RESPONSE] responding to `sign` with: {:?}.", response)?;
        self.state.lock().currently_signing = false;
        Ok(response)
    }

    /// Gets the mnemonic stored on disk for a specific `network` for front-end export
    /// requiring password authorization.
    #[inline]
    pub async fn get_stored_mnemonic(
        &mut self,
        network: Network,
        prompt: &String,
    ) -> Result<Mnemonic> {
        self.authorizer.lock().await.check(prompt).await?;
        let stored_mnemonic = self.state.lock().signer[network]
            .state()
            .accounts()
            .as_ref()
            .expect("No accounts in signer")
            .keys()
            .expose_mnemonic()
            .clone();
        Ok(stored_mnemonic)
    }

    /// Runs the receiving key sampling protocol on the signer.
    #[inline]
    pub async fn address(self, request: ReceivingKeyRequest) -> Result<Address> {
        let response = self.state.lock().signer[request.network]
            .address()
            .expect("No address present in signer!");
        info!("[RESPONSE] responding to `receivingKeys` with: {response:?}")?;
        Ok(response)
    }

    /// Runs the receiving key sampling protocol on a mutable reference of the signer, and formats
    /// the result to base 58.
    #[inline]
    pub async fn get_address(&mut self, request: ReceivingKeyRequest) -> Result<String, ()> {
        let response = self.state.lock().signer[request.network]
            .address()
            .ok_or(())?;
        let key = address_to_base58(&response);
        Ok(key)
    }
}
