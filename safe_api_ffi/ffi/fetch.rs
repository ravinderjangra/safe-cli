use super::ffi_structs::{FilesContainer, PublishedImmutableData, SafeKey, Wallet, files_map_into_repr_c, wallet_spendable_balances_into_repr_c};
use super::helpers::to_c_str;
use super::{ResultReturn, Safe};
use ffi_utils::{catch_unwind_cb, from_c_str, FfiResult, OpaqueCtx};
use safe_api::fetch::SafeData;
use std::os::raw::{c_char, c_void};

#[no_mangle]
pub unsafe extern "C" fn fetch(
    app: *mut Safe,
    url: *const c_char,
    user_data: *mut c_void,
    o_published: extern "C" fn(user_data: *mut c_void, *const PublishedImmutableData),
    o_wallet: extern "C" fn(user_data: *mut c_void, *const Wallet),
    o_keys: extern "C" fn(user_data: *mut c_void, *const SafeKey),
    o_container: extern "C" fn(user_data: *mut c_void, *const FilesContainer),
    o_err: extern "C" fn(user_data: *mut c_void, result: *const FfiResult),
) {
    catch_unwind_cb(user_data, o_err, || -> ResultReturn<()> {
        let user_data = OpaqueCtx(user_data);
        let url = from_c_str(url)?;
        let content = (*app).fetch(&url)?;
        match &content {
            SafeData::PublishedImmutableData {
                data,
                xorname,
                resolved_from,
                media_type,
            } => {
                let resolved_from_json = serde_json::to_string(&resolved_from)?;
                let published_data = PublishedImmutableData {
                    xorname: xorname.0,
                    data: data.as_ptr(),
                    data_len: data.len(),
                    resolved_from: to_c_str(resolved_from_json)?.as_ptr(),
                    media_type: to_c_str(media_type.clone().unwrap())?.as_ptr(),
                };
                o_published(user_data.0, &published_data);
            }
            SafeData::FilesContainer {
                version,
                files_map,
                type_tag,
                xorname,
                data_type,
                resolved_from,
            } => {
                let resolved_from_json = serde_json::to_string(&resolved_from)?;
                let container = FilesContainer {
                    version: *version,
                    files_map: &files_map_into_repr_c(&files_map)?,
                    type_tag: *type_tag,
                    xorname: xorname.0,
                    data_type: (*data_type).clone() as u64,
                    resolved_from: to_c_str(resolved_from_json)?.as_ptr(),
                };
                o_container(user_data.0, &container);
            }
            SafeData::Wallet {
                xorname,
                type_tag,
                balances,
                data_type,
                resolved_from,
            } => {
                let resolved_from_json = serde_json::to_string(&resolved_from)?;
                let wallet = Wallet {
                    xorname: xorname.0,
                    type_tag: *type_tag,
                    balances: &wallet_spendable_balances_into_repr_c(balances)?,
                    data_type: (*data_type).clone() as u64,
                    resolved_from: to_c_str(resolved_from_json)?.as_ptr(),
                };
                o_wallet(user_data.0, &wallet);
            }
            SafeData::SafeKey {
                xorname,
                resolved_from,
            } => {
                let resolved_from_json = serde_json::to_string(&resolved_from)?;
                let keys = SafeKey {
                    xorname: xorname.0,
                    resolved_from: to_c_str(resolved_from_json)?.as_ptr(),
                };
                o_keys(user_data.0, &keys);
            }
        };
        Ok(())
    })
}
