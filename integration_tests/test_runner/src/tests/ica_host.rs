use std::time::{Duration, Instant};

use althea_proto::althea::microtx::v1::MsgMicrotx;
use althea_proto::althea_test::gaia::icaauth::v1::{MsgRegisterAccount, MsgSubmitTx};
use althea_proto::cosmos_sdk_proto::cosmos::params::v1beta1::{
    ParamChange, ParameterChangeProposal,
};
use althea_proto::cosmos_sdk_proto::ibc::core::channel::v1::query_client::QueryClient as IbcChannelQueryClient;
use althea_proto::cosmos_sdk_proto::{
    cosmos::base::abci::v1beta1::TxResponse,
    ibc::applications::interchain_accounts::{
        controller::v1::{
            query_client::QueryClient as ICAControllerQueryClient, QueryInterchainAccountRequest,
            QueryParamsRequest as ControllerQueryParamsRequest,
        },
        host::v1::{
            query_client::QueryClient as ICAHostQueryClient,
            QueryParamsRequest as HostQueryParamsRequest,
        },
    },
};
use deep_space::client::type_urls::MSG_MICROTX_TYPE_URL;
use deep_space::error::CosmosGrpcError;
use deep_space::{Address, Coin, Contact, CosmosPrivateKey, Msg, PrivateKey};
use tokio::time::sleep;
use tonic::transport::Channel;

use crate::utils::{
    encode_any, footoken_metadata, get_fee, one_atom, vote_yes_on_proposals,
    wait_for_proposals_to_execute, ValidatorKeys, ADDRESS_PREFIX, IBC_ADDRESS_PREFIX,
    IBC_STAKING_TOKEN, OPERATION_TIMEOUT, STAKING_TOKEN, TOTAL_TIMEOUT,
};
use crate::{
    ibc_utils::get_channel,
    utils::{get_ibc_chain_id, COSMOS_NODE_GRPC, IBC_NODE_GRPC},
};

pub const MSG_REGISTER_ACCOUNT_TYPE_URL: &str = "/gaia.icaauth.v1.MsgRegisterAccount";
pub const MSG_SUBMIT_TX_TYPE_URL: &str = "/gaia.icaauth.v1.MsgSubmitTx";

/// Runs the "happy-path" functionality of the Interchain Accounts (ICA) Host Module on Althea-L1:
/// 1. Enable Host on Althea-L1 and Controller on the IBC test chain
/// 2. Register an Interchain Account controlled by the IBC test chain and fund it with footoken
/// 3. Submit a MsgMicrotx via the ICA paying in footoken and confirm the transaction was successful
pub async fn ica_host_happy_path(
    althea_contact: &Contact,
    ibc_contact: &Contact,
    keys: Vec<ValidatorKeys>,
    ibc_keys: Vec<CosmosPrivateKey>,
) {
    let althea_channel_qc = IbcChannelQueryClient::connect(COSMOS_NODE_GRPC.as_str())
        .await
        .expect("Could not connect channel query client");
    let _ica_channel_qc = IbcChannelQueryClient::connect(IBC_NODE_GRPC.as_str())
        .await
        .expect("Could not connect channel query client");
    let ica_controller_qc = ICAControllerQueryClient::connect(IBC_NODE_GRPC.as_str())
        .await
        .expect("Cound not connect ica controller query client");

    // Wait for the ibc channel to be created and find the channel ids
    info!("Waiting for IBC channel creation:");
    let channel_id_timeout = Duration::from_secs(60 * 5);
    let althea_channel = get_channel(
        althea_channel_qc,
        get_ibc_chain_id(),
        Some(channel_id_timeout),
    )
    .await
    .expect("Could not find ibc-test-1 channel");
    let ibc_to_althea_connection_id = althea_channel.connection_hops[0].clone();

    info!("\n\n!!!!!!!!!! Start ICA Host Happy Path Test !!!!!!!!!!\n\n");
    enable_ica_host(althea_contact, &keys).await;
    enable_ica_controller(ibc_contact, &keys).await;

    let ibc_fee = Coin {
        amount: 1u8.into(),
        denom: IBC_STAKING_TOKEN.to_string(),
    };
    let zero_fee = Coin {
        amount: 0u8.into(),
        denom: STAKING_TOKEN.to_string(),
    };

    let ica_owner = ibc_keys[0];
    let ica_owner_addr = ica_owner.to_address(&IBC_ADDRESS_PREFIX).unwrap();
    let ica_addr: String = get_or_register_ica(
        ibc_contact,
        ica_controller_qc.clone(),
        ica_owner,
        ica_owner_addr.to_string(),
        ibc_to_althea_connection_id.clone(),
        ibc_fee.clone(),
    )
    .await
    .expect("Could not get/register interchain account");
    let ica_address = Address::from_bech32(ica_addr).expect("invalid interchain account address?");

    info!("Funding interchain account");
    let fund_amt = Coin {
        amount: one_atom(),
        denom: STAKING_TOKEN.to_string(),
    };
    althea_contact
        .send_coins(
            fund_amt,
            Some(zero_fee),
            ica_address,
            Some(OPERATION_TIMEOUT),
            keys[0].validator_key,
        )
        .await
        .expect("Failed to fund ICA");

    let footoken = footoken_metadata(althea_contact).await;
    let send_to_ica_coin = Coin {
        amount: one_atom(),
        denom: footoken.base,
    };

    // send the ica user some footoken
    althea_contact
        .send_coins(
            send_to_ica_coin.clone(),
            Some(get_fee(None)),
            ica_address,
            Some(TOTAL_TIMEOUT),
            keys[0].validator_key,
        )
        .await
        .unwrap();

    send_microtx_via_ica_and_confirm(
        ibc_contact,
        althea_contact,
        ica_owner,
        ica_address,
        ibc_to_althea_connection_id,
        keys[1].validator_key.to_address(&ADDRESS_PREFIX).unwrap(),
        send_to_ica_coin,
    )
    .await;
    info!("Successful ICA Host Happy Path Test");
}

// ---------------------------------------HELPER FUNCTIONS---------------------------------------

/// Submits a MsgRegisterAccount to x/icaauth to create an account over `connection_id` for `owner`
pub async fn register_interchain_account(
    contact: &Contact,
    owner_key: impl PrivateKey,
    owner: String,
    connection_id: String,
    fee: Coin,
) -> Result<TxResponse, CosmosGrpcError> {
    let register = MsgRegisterAccount {
        owner,
        connection_id,
        version: String::new(),
    };
    let register_msg = Msg::new(MSG_REGISTER_ACCOUNT_TYPE_URL.to_string(), register);
    contact
        .send_message(
            &[register_msg],
            None,
            &[fee],
            Some(OPERATION_TIMEOUT),
            owner_key,
        )
        .await
}

/// Queries x/icaauth for `owner`'s interchain account over `connection_id`
pub async fn get_interchain_account_address(
    ica_controller_qc: ICAControllerQueryClient<Channel>,
    owner: String,
    connection_id: String,
    timeout: Option<Duration>,
) -> Result<String, CosmosGrpcError> {
    let timeout = timeout.unwrap_or(OPERATION_TIMEOUT);
    let start = Instant::now();
    let mut ica_controller_qc = ica_controller_qc;
    while Instant::now() - start < timeout {
        let res = ica_controller_qc
            .interchain_account(QueryInterchainAccountRequest {
                owner: owner.clone(),
                connection_id: connection_id.clone(),
            })
            .await
            .map(|r| r.into_inner().address)
            .map_err(|e| CosmosGrpcError::BadResponse(e.to_string()));
        if res.is_ok() {
            return res;
        }
        sleep(Duration::from_secs(5)).await;
    }
    Err(CosmosGrpcError::BadResponse(format!(
        "Failed to get account after {timeout:?}"
    )))
}

/// Either locates an already registered interchain account or creates one for the given `ctrlr`, over connection `ctrl_to_host_conn_id`
pub async fn get_or_register_ica(
    ctrl_contact: &Contact,
    ctrl_qc: ICAControllerQueryClient<Channel>,
    ctrlr_key: impl PrivateKey,
    ctrlr_addr: String,
    ctrl_to_host_conn_id: String,
    fee: Coin,
) -> Result<String, CosmosGrpcError> {
    info!("Finding/Making ICA for {ctrlr_addr} on connection {ctrl_to_host_conn_id}");
    let ica_addr: String;
    let ica_already_exists = get_interchain_account_address(
        ctrl_qc.clone(),
        ctrlr_addr.to_string(),
        ctrl_to_host_conn_id.clone(),
        Some(OPERATION_TIMEOUT),
    )
    .await;
    let ica = if let Ok(ica_addr) = ica_already_exists {
        info!("Interchain account {ica_addr} already registered");
        ica_addr
    } else {
        let register_res = register_interchain_account(
            ctrl_contact,
            ctrlr_key.clone(),
            ctrlr_addr.clone(),
            ctrl_to_host_conn_id.clone(),
            fee.clone(),
        )
        .await?;
        info!("Registered Interchain Account: {}", register_res.raw_log);

        ica_addr = get_interchain_account_address(
            ctrl_qc.clone(),
            ctrlr_addr,
            ctrl_to_host_conn_id.clone(),
            Some(TOTAL_TIMEOUT),
        )
        .await?;
        info!("Discovered interchain account with address {ica_addr:?}");
        ica_addr
    };
    Ok(ica)
}

/// Creates and ratifies a ParameterChangeProposal to enable the ICA Host module and allow all messages
/// Note: Skips governance if the host module is already enabled
pub async fn enable_ica_host(
    contact: &Contact, // Src chain's deep_space client
    keys: &[ValidatorKeys],
) {
    let mut host_qc = ICAHostQueryClient::connect(contact.get_url())
        .await
        .expect("Unable to connect to ica host query client");
    let host_params = host_qc
        .params(HostQueryParamsRequest {})
        .await
        .unwrap()
        .into_inner()
        .params
        .expect("No ica host params returned?");
    if host_params.host_enabled
        && host_params
            .allow_messages
            .first()
            .map(|m| m == "*")
            .unwrap_or(false)
    {
        info!("ICA Host already enabled, skipping governance vote");
        return;
    } else {
        info!("Host params are {host_params:?}: Enabling ICA Host via governance, will set AllowMessages to [\"*\"]");
    }

    let deposit = Coin {
        amount: one_atom() * 100u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let fee = Coin {
        amount: 0u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let res = contact
        .submit_parameter_change_proposal(
            ParameterChangeProposal {
                title: "Enable ICA Host".to_string(),
                description: "Enable ICA Host".to_string(),
                changes: vec![
                    // subspace defined at ibc-go/modules/apps/27-interchain-accounts/host/types/keys.go
                    // keys defined at     ibc-go/modules/apps/27-interchain-accounts/host/types/params.go
                    ParamChange {
                        subspace: "icahost".to_string(),
                        key: "HostEnabled".to_string(),
                        value: "true".to_string(),
                    },
                    ParamChange {
                        subspace: "icahost".to_string(),
                        key: "AllowMessages".to_string(),
                        value: "[\"*\"]".to_string(),
                    },
                ],
            },
            deposit,
            fee,
            keys[0].validator_key,
            Some(OPERATION_TIMEOUT),
        )
        .await;
    vote_yes_on_proposals(contact, keys, None).await;
    wait_for_proposals_to_execute(contact).await;
    trace!("Gov proposal executed with {:?}", res);
}

/// Creates and ratifies a ParameterChangeProposal to enable the ICA Controller module
pub async fn enable_ica_controller(
    contact: &Contact, // Src chain's deep_space client
    keys: &[ValidatorKeys],
) {
    let mut controller_qc = ICAControllerQueryClient::connect(contact.get_url())
        .await
        .expect("Unable to connect to ica controller query client");
    let controller_params = controller_qc
        .params(ControllerQueryParamsRequest {})
        .await
        .unwrap()
        .into_inner()
        .params
        .expect("No ica controller params returned?");
    if controller_params.controller_enabled {
        info!("ICA Controller already enabled, skipping governance vote");
        return;
    } else {
        info!("Enabling ICA Controller via governance");
    }

    let deposit = Coin {
        amount: one_atom() * 100u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let fee = Coin {
        amount: 0u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let res = contact
        .submit_parameter_change_proposal(
            ParameterChangeProposal {
                title: "Enable ICA Controller".to_string(),
                description: "Enable ICA Controller".to_string(),
                changes: vec![
                    // subspace defined at ibc-go/modules/apps/27-interchain-accounts/controller/types/keys.go
                    // keys defined at     ibc-go/modules/apps/27-interchain-accounts/controller/types/params.go
                    ParamChange {
                        subspace: "icacontroller".to_string(),
                        key: "icacontroller".to_string(),
                        value: "true".to_string(),
                    },
                ],
            },
            deposit,
            fee,
            keys[0].validator_key,
            Some(OPERATION_TIMEOUT),
        )
        .await;
    vote_yes_on_proposals(contact, keys, None).await;
    wait_for_proposals_to_execute(contact).await;
    trace!("Gov proposal executed with {:?}", res);
}

pub async fn send_microtx_via_ica_and_confirm(
    ctrl_contact: &Contact,
    host_contact: &Contact,
    ica_owner: CosmosPrivateKey,
    ica_address: Address,
    ctrl_to_host_conn_id: String,
    microtx_receiver: Address,
    amount: Coin,
) -> bool {
    let denom = amount.denom.clone();
    let recv_pre_balance = host_contact
        .get_balance(microtx_receiver, denom.clone())
        .await
        .expect("Unable to get microtx receiver balance")
        .unwrap_or(Coin {
            denom: denom.clone(),
            amount: 0u8.into(),
        });

    send_microtx_via_ica(
        ctrl_contact,
        ica_owner,
        ica_address,
        ctrl_to_host_conn_id,
        microtx_receiver,
        amount,
    )
    .await
    .expect("Failed to send microtx via ica");

    let start = Instant::now();
    // overly complicated retry logic allows us to handle the possibility that gas prices change between blocks
    // and cause any individual request to fail.

    while Instant::now() - start < TOTAL_TIMEOUT {
        sleep(Duration::from_secs(5)).await;
        let recv_post_balance = host_contact
            .get_balance(microtx_receiver, denom.clone())
            .await;
        match recv_post_balance {
            Ok(Some(recv_post_balance)) => {
                if recv_post_balance.amount > recv_pre_balance.amount {
                    info!("Successfully sent microtx via ICA");
                    return true;
                }
                continue;
            }
            _ => continue,
        };
    }
    false
}

pub async fn send_microtx_via_ica(
    ctrl_contact: &Contact,
    ica_owner: CosmosPrivateKey,
    ica_address: Address,
    ctrl_to_host_conn_id: String,
    microtx_receiver: Address,
    amount: Coin,
) -> Result<TxResponse, CosmosGrpcError> {
    let msg_microtx = MsgMicrotx {
        sender: ica_address.to_string(),
        receiver: microtx_receiver.to_string(),
        amount: Some(amount.into()),
    };
    info!("Sending Microtx via ICA: {:?}", msg_microtx);
    let msg = encode_any(msg_microtx, MSG_MICROTX_TYPE_URL);

    let ica_submit = MsgSubmitTx {
        connection_id: ctrl_to_host_conn_id,
        owner: ica_owner
            .to_address(&IBC_ADDRESS_PREFIX)
            .unwrap()
            .to_string(),
        msgs: vec![msg],
    };
    info!("Submitting MsgSubmitTx: {ica_submit:?}");
    let ica_msg = Msg::new(MSG_SUBMIT_TX_TYPE_URL, ica_submit);
    let ctrl_fee = Coin {
        amount: 100u8.into(),
        denom: IBC_STAKING_TOKEN.to_string(),
    };
    ctrl_contact
        .send_message(
            &[ica_msg],
            None,
            &[ctrl_fee],
            Some(OPERATION_TIMEOUT),
            ica_owner,
        )
        .await
}
