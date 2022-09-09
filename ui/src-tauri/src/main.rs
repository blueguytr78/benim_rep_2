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

//! Manta Signer UI

#![cfg_attr(doc_cfg, feature(doc_cfg))]
#![forbid(rustdoc::broken_intra_doc_links)]
#![forbid(missing_docs)]
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

extern crate alloc;

use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use manta_signer::{
    config::{Config, Setup},
    secret::{
        password_channel, Authorizer, Password, PasswordFuture, PasswordReceiver, PasswordSender,
        Secret, SetupFuture, UnitFuture,
    },
    serde::Serialize,
    service::Server,
    storage::Store,
};
use std::time::Instant;
use tauri::{
    async_runtime::spawn, CustomMenuItem, Manager, RunEvent, Runtime, State, SystemTray,
    SystemTrayEvent, SystemTrayMenu, Window, WindowEvent,
};

/// App State
///
/// Keeps track of global state flags that we need for specific behaviors.
#[derive(Debug)]
pub struct AppState {
    /// UI is Connected
    pub ui_connected: AtomicBool,

    /// Currently Authorising
    pub authorizing: AtomicBool,
}

impl AppState {
    /// Builds a new [`AppState`].
    #[inline]
    pub const fn new() -> Self {
        Self {
            ui_connected: AtomicBool::new(false),
            authorizing: AtomicBool::new(false),
        }
    }

    /// Returns the UI connection status.
    #[inline]
    pub fn get_ui_connected(&self) -> bool {
        self.ui_connected.load(Ordering::Relaxed)
    }

    /// Sets the UI connection status.
    #[inline]
    pub fn set_ui_connected(&self, ui_connected: bool) {
        self.ui_connected.store(ui_connected, Ordering::Relaxed)
    }

    /// Returns the authorizing status.
    #[inline]
    pub fn get_authorizing(&self) -> bool {
        self.authorizing.load(Ordering::Relaxed)
    }

    /// Sets the authorizing status.
    #[inline]
    pub fn set_authorizing(&self, auth: bool) {
        self.authorizing.store(auth, Ordering::Relaxed);
    }
}

/// Application State
pub static APP_STATE: AppState = AppState::new();

/// Repeatedly executes `f` until the `timeout` is reached calling `exit` to return from the
/// function.
#[inline]
pub fn while_timeout<F, E, T>(timeout: Duration, mut f: F, exit: E) -> T
where
    F: FnMut(),
    E: FnOnce(Instant, Duration) -> T,
{
    let time_start = Instant::now();
    loop {
        f();
        if time_start.elapsed() >= timeout {
            return exit(time_start, timeout);
        }
    }
}

/// User
pub struct User {
    /// Main Window
    window: Window,

    /// Password Receiver
    password_receiver: RetryReceiver<Password>,

    /// Password Receiver
    password_receiver: RetryReceiver<Mnemonic>,

    /// Waiting Flag
    waiting: bool,
}

impl User {
    /// Builds a new [`User`] from `window` and `password_receiver`.
    #[inline]
    pub fn new(window: Window, password_receiver: PasswordReceiver) -> Self {
        Self {
            window,
            password_receiver,
            waiting: false,
        }
    }

    /// Emits a `message` of the given `kind` to the window.
    #[inline]
    fn emit<T>(&self, kind: &'static str, message: &T)
    where
        T: Serialize,
    {
        self.window
            .emit(kind, message)
            .expect("Unable to emit message to the window.")
    }

    /// Requests password from user, sending a retry message if the previous password did not match
    /// correctly.
    #[inline]
    async fn request_password(&mut self) -> Password {
        if self.waiting {
            self.password_receiver.should_retry(true).await;
        }
        let password = self.password_receiver.password().await;
        self.waiting = password.is_known();
        password
    }

    /// Sends validation message when password was correctly matched.
    #[inline]
    async fn validate_password(&mut self) {
        self.waiting = false;
        self.password_receiver.should_retry(false).await;
    }
}

impl Authorizer for User {
    #[inline]
    fn password(&mut self) -> PasswordFuture {
        Box::pin(async move { self.request_password().await })
    }

    #[inline]
    fn setup<'s>(&'s mut self, data_exists: bool) -> SetupFuture<'s> {
        let window = self.window.clone();
        Box::pin(async move {
            /*
            while_timeout(
                Duration::from_millis(5000),
                move || {
                    if APP_STATE.get_ui_connected() {
                        return;
                    }
                    window
                        .emit("connect", data_exists)
                        .expect("The `connect` command failed to be emitted to the window.");
                },
                move |time_start, timeout| {
                    panic!(
                        "Connection attempt timed-out! Started: {:?} with {:?} timeout.",
                        time_start, timeout
                    );
                },
            )
            */

            1. emit the connect message
            2. branch on `data_exists` flag
            3. if true then wait on the mnemonic channel
                4. once we receive the mnemonic we set the `Setup` value to it
                5. if bad mnemonic send retry and jump to 3
            5. otherwise we just return immediately with the `Login` value


        })
    }

    #[inline]
    fn wake<T>(&mut self, prompt: &T) -> UnitFuture
    where
        T: Serialize,
    {
        APP_STATE.set_authorizing(true);
        self.emit("authorize", prompt);
        Box::pin(async move {})
    }

    #[inline]
    fn sleep(&mut self) -> UnitFuture {
        APP_STATE.set_authorizing(false);
        Box::pin(async move { self.validate_password().await })
    }
}

/// Mnemonic Store
pub type MnemonicStore = Store<MnemonicSender>;

/// Password Store
pub type PasswordStore = Store<PasswordSender>;

/// Server Store
pub type ServerStore = Store<Server<User>>;

/// Called from the UI after it recieves a `connect` event.
///
/// To ensure proper connection you should emit `connect` continuously until the
/// [`AppState::ui_connected`] flag is `true` then stop. This is the only way for now to ensure they
/// are synchronized. Tauri is working on a better way.
#[tauri::command]
fn ui_connected() {
    APP_STATE.set_ui_connected(true);
}

/// Sends the current `mnemonic` into storage from the UI.
#[tauri::command]
async fn send_mnemonic(
    mnemonic_store: State<'_, MnemonicStore>,
    mnemonic: String,
) -> Result<bool, ()> {
    if let Some(store) = &mut *mnemonic_store.lock().await {
        Ok(store.load(Secret::new(mnemonic)).await)
    } else {
        Ok(false)
    }
}

/// Sends the current `password` into storage from the UI.
#[tauri::command]
async fn send_password(
    password_store: State<'_, PasswordStore>,
    password: String,
) -> Result<bool, ()> {
    if let Some(store) = &mut *password_store.lock().await {
        Ok(store.load(Secret::new(password)).await)
    } else {
        Ok(false)
    }
}

/// Stops the server from prompting for the password.
#[tauri::command]
async fn stop_password_prompt(password_store: State<'_, PasswordStore>) -> Result<(), ()> {
    if let Some(store) = &mut *password_store.lock().await {
        store.clear().await;
    }
    Ok(())
}

///
#[tauri::command]
async fn restart(
    server: State<'_, ServerStore>,
    abort_handle: State<'_, ServerAbortHandleStore>
) -> Result<(), ()> {
    abort_handle.lock().await.abort();
    let handle = spawn(async {
        server.lock().start().await
    });
    abort_handle.set(handle);
}

/// Returns the window with the given `label` from `app`.
///
/// # Panics
///
/// This function panics if the window with the given `label` was unreachable.
#[inline]
pub fn window<R, M>(app: &M, label: &str) -> Window<R>
where
    R: Runtime,
    M: Manager<R>,
{
    match app.get_window(label) {
        Some(window) => window,
        _ => panic!("Unable to get {:?} window handler.", label),
    }
}

/// Runs the main Tauri application.
fn main() {
    let config =
        Config::try_default().expect("Unable to generate the default server configuration.");

    let mut app = tauri::Builder::default()
        .system_tray(
            SystemTray::new().with_menu(
                SystemTrayMenu::new()
                    .add_item(CustomMenuItem::new("about", "About"))
                    .add_item(CustomMenuItem::new("exit", "Quit")),
            ),
        )
        .on_system_tray_event(move |app, event| {
            if let SystemTrayEvent::MenuItemClick { id, .. } = event {
                match id.as_str() {
                    "about" => window(app, "about").show().expect("Unable to show window."),
                    "exit" => app.exit(0),
                    _ => {}
                }
            }
        })
        .manage(PasswordStore::default())
        .manage(ServerStore::default())
        .setup(|app| {
            let window = window(app, "main");
            let password_store = app.state::<PasswordStore>().inner().clone();
            let server_store = app.state::<ServerStore>().inner().clone();

            let server_abort_handle_store =
                app.state::<ServerAbortHandleStore>().inner().clone();

            let handle = spawn(async move {
                let (password_sender, password_receiver) = password_channel();
                password_store.set(password_sender).await;
                let server = Server::build(config, User::new(window, password_receiver))
                    .await
                    .expect("Unable to build manta-signer server.");
                server_store.set(server.clone()).await;
                server
                    .start()
                    .await
                    .expect("Unable to build manta-signer service.");
            });

            server_abort_handle_store.set(handle);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            send_password,
            stop_password_prompt,
            ui_connected,
        ])
        .build(tauri::generate_context!())
        .expect("Error while building UI.");

    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    app.run(|app, event| match event {
        RunEvent::Ready => window(app, "about").hide().expect("Unable to hind window."),
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } => {
            api.prevent_close();
            match label.as_str() {
                "about" => window(app, "about").hide().expect("Unable to hide window."),
                "main" => {
                    if APP_STATE.get_authorizing() {
                        window(app, "main").hide().expect("Unable to hide window.");
                        window(app, "main")
                            .emit("abort_auth", "Aborting Authorization")
                            .expect("Failed to abort authorization");
                    } else {
                        app.exit(0);
                    }
                }
                _ => unreachable!("There are no other windows."),
            }
        }
        _ => (),
    })
}
