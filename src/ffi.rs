//! FFI bindings to the language-neutral `lp_*` C ABI (logos-protocol).
//!
//! Since the protocol extraction, the Rust SDK consumes logos-protocol
//! directly instead of logos-module-client's `logos_sdk_*` facade. The
//! symbols resolve at final link time when CMake links the module plugin
//! against the protocol archive (chained through logos-qt-sdk by
//! logos-module-builder's plugin link line).

use std::ffi::{c_char, c_int, c_void};

/// Opaque client handle (`lp_client*`).
#[repr(C)]
pub struct LpClient {
    _private: [u8; 0],
}

/// Opaque subscription handle (`lp_subscription*`).
#[repr(C)]
pub struct LpSubscription {
    _private: [u8; 0],
}

/// Result callback for `lp_invoke_async`:
/// `ok != 0` → `json` is the result JSON value; `ok == 0` → canonical error
/// object. `json` is only valid for the duration of the callback.
pub type LpResultCb = extern "C" fn(ok: c_int, json: *const c_char, user_data: *mut c_void);

/// Event callback for `lp_subscribe`. `data_json` is a JSON array payload.
pub type LpEventCb =
    extern "C" fn(event_name: *const c_char, data_json: *const c_char, user_data: *mut c_void);

/// Subscription status callback for `lp_client_set_subscription_status_cb`
/// (logos-protocol 0.9+). Reports the TARGET MODULE's transitions, not one
/// subscription's: every subscription through a client shares the provider's
/// single handle, so they are lost and re-established together.
///
/// `reason` is a lowercase snake_case code on LOST/HELD/ABANDONED, NULL on ARMED.
pub type LpSubStatusCb = extern "C" fn(
    state: c_int,
    generation: u64,
    reason: *const c_char,
    user_data: *mut c_void,
);

/// Subscription status codes carried by [`LpSubStatusCb`].
pub const LP_SUB_ARMED: c_int = 1;
pub const LP_SUB_LOST: c_int = 2;
pub const LP_SUB_ABANDONED: c_int = 3;
/// The provider went away AND the target's restart policy is manual, so nothing
/// will re-arm itself. Delivered INSTEAD OF `LP_SUB_LOST`.
pub const LP_SUB_HELD: c_int = 4;

pub const LP_OK: c_int = 0;

extern "C" {
    pub fn lp_protocol_version() -> *const c_char;
    pub fn lp_protocol_abi_major() -> c_int;

    pub fn lp_string_free(s: *mut c_char);

    /// Route a host-issued grant into THIS image's gate state.
    ///
    /// It has to cross the C ABI rather than be recorded once by the host: the
    /// host binary and this cdylib each link their own copy of logos-protocol,
    /// so each has its own process-global grant state, exactly as each has its
    /// own TokenManager. A grant the host records for itself is invisible to
    /// the gate an lp_token_keys() call checks here.
    ///
    /// Added in logos-protocol 0.3; see `logos_module_grant_host_services` in
    /// lidl-gen's provider scaffold, which is the only caller.
    pub fn lp_grant_host_services(services_json: *const c_char) -> c_int;

    pub fn lp_client_create(
        target_module: *const c_char,
        origin_module: *const c_char,
        target_transport_json: *const c_char,
        capability_transport_json: *const c_char,
    ) -> *mut LpClient;
    pub fn lp_client_destroy(client: *mut LpClient);

    pub fn lp_invoke(
        client: *mut LpClient,
        method: *const c_char,
        args_json: *const c_char,
        timeout_ms: c_int,
        out_result_json: *mut *mut c_char,
        out_error_json: *mut *mut c_char,
    ) -> c_int;

    pub fn lp_invoke_async(
        client: *mut LpClient,
        method: *const c_char,
        args_json: *const c_char,
        timeout_ms: c_int,
        cb: LpResultCb,
        user_data: *mut c_void,
    ) -> c_int;

    pub fn lp_token_save(module_name: *const c_char, token: *const c_char) -> c_int;

    // THE INBOUND DOOR (logos-protocol 0.8, lp_token_save_inbound) is
    // DELIBERATELY NOT DECLARED HERE. It is declared inside the generated
    // provider scaffold, under the same 0.8 gate as the export that calls it --
    // see lidl-gen's accept_inbound_token_block.
    //
    // The reason is a link requirement, not taste. A wrapper in api.rs sits in
    // the same rlib object as save_token, which every module needs, so the
    // linker pulls the object in and the undefined lp_token_save_inbound
    // reference with it -- for EVERY module, including ones generated for
    // protocol 0.7 that emit no call at all. Measured: every Rust module died
    // at dlopen with "undefined symbol: lp_token_save_inbound", on Linux only.
    // Keeping the binding inside the gated block is what makes landing this
    // ahead of the protocol bump inert.

    pub fn lp_subscribe(
        client: *mut LpClient,
        event_name: *const c_char,
        cb: LpEventCb,
        user_data: *mut c_void,
    ) -> *mut LpSubscription;
    pub fn lp_unsubscribe(sub: *mut LpSubscription);

    // 0.9+ / 0.10+.
    //
    // DECLARING these is free — an unused `extern "C"` declaration emits no
    // symbol reference. It is a CALL SITE inside this crate that stamps an
    // unconditional undefined symbol into every Rust module's staticlib,
    // whether or not that module uses the feature. Measured: a shipped
    // libkeystore_module.a has undefined `_lp_subscribe` but NOT
    // `_lp_get_methods`, which is declared here and called by nothing.
    //
    // So the declarations are unconditional and the CALL SITES are gated —
    // see PluginProxy's subscription-state methods. Rust does not check a
    // hand-written declaration
    // against the real symbol, which is also why logos-protocol only ever ADDS
    // functions here rather than changing one: a stale arity would link against
    // the same name and mis-call it with no diagnostic anywhere.
    pub fn lp_client_set_subscription_status_cb(
        client: *mut LpClient,
        status_cb: Option<LpSubStatusCb>,
        user_data: *mut c_void,
    ) -> c_int;
    pub fn lp_client_set_subscription_options(
        client: *mut LpClient,
        options_json: *const c_char,
    ) -> c_int;
    pub fn lp_client_subscription_generation(client: *mut LpClient) -> u64;
    pub fn lp_client_rearm_subscriptions(client: *mut LpClient) -> c_int;

    pub fn lp_get_methods(client: *mut LpClient) -> *mut c_char;

    /// Local presence of the client's target: 0 unknown, 1 present, 2 absent.
    /// Never blocks and never calls the peer. Unknown means "try the call",
    /// never "absent" — see lp_target_presence in logos_protocol.h.
    pub fn lp_target_presence(client: *mut LpClient) -> c_int;
}

pub const LP_PRESENCE_UNKNOWN: c_int = 0;
pub const LP_PRESENCE_PRESENT: c_int = 1;
pub const LP_PRESENCE_ABSENT: c_int = 2;
