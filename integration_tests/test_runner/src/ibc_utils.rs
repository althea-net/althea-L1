use crate::utils::*;
use althea_proto::cosmos_sdk_proto::cosmos::bank::v1beta1::query_client::QueryClient as BankQueryClient;
use althea_proto::cosmos_sdk_proto::cosmos::bank::v1beta1::QueryBalanceRequest;
use althea_proto::cosmos_sdk_proto::cosmos::base::abci::v1beta1::TxResponse;
use althea_proto::cosmos_sdk_proto::ibc::applications::transfer::v1::query_client::QueryClient as IbcTransferQueryClient;
use althea_proto::cosmos_sdk_proto::ibc::applications::transfer::v1::{
    DenomTrace, MsgTransfer, QueryDenomHashRequest, QueryDenomTraceRequest,
};
use althea_proto::cosmos_sdk_proto::ibc::core::channel::v1::query_client::QueryClient as IbcChannelQueryClient;
use althea_proto::cosmos_sdk_proto::ibc::core::channel::v1::IdentifiedChannel;
use althea_proto::cosmos_sdk_proto::ibc::core::channel::v1::{
    QueryChannelClientStateRequest, QueryChannelsRequest,
};
use althea_proto::cosmos_sdk_proto::ibc::lightclients::tendermint::v1::ClientState;
use clarity::Uint256;
use deep_space::client::type_urls::MSG_TRANSFER_TYPE_URL;
use deep_space::error::CosmosGrpcError;
use deep_space::utils::decode_any;
use deep_space::{Address, Coin, Contact, Msg, PrivateKey};
use std::ops::Add;
use std::time::Instant;
use std::time::{Duration, SystemTime};
use tokio::time::sleep as delay_for;
use tonic::transport::Channel;

// Retrieves the channel connecting the chain behind `ibc_channel_qc` and the chain with id `foreign_chain_id`
// Retries up to `timeout` (or OPERATION_TIMEOUT if None), checking each channel's client state to find the foreign chain's id
pub async fn get_channel(
    ibc_channel_qc: IbcChannelQueryClient<Channel>, // The Src chain's IbcChannelQueryClient
    foreign_chain_id: String,                       // The chain-id of the Dst chain
    timeout: Option<Duration>,
) -> Result<IdentifiedChannel, CosmosGrpcError> {
    let mut ibc_channel_qc = ibc_channel_qc;
    let timeout = match timeout {
        Some(t) => t,
        None => OPERATION_TIMEOUT,
    };

    let start = Instant::now();
    while Instant::now() - start < timeout {
        let channels = ibc_channel_qc
            .channels(QueryChannelsRequest { pagination: None })
            .await;
        if channels.is_err() {
            delay_for(Duration::from_secs(5)).await;
            continue;
        }
        let channels = channels.unwrap().into_inner().channels;
        for channel in channels {
            // Make an IBC Channel ClientState request with port=transfer channel=channel.channel_id
            let client_state_res = ibc_channel_qc
                .channel_client_state(QueryChannelClientStateRequest {
                    port_id: "transfer".to_string(),
                    channel_id: channel.channel_id.clone(),
                })
                .await;
            if client_state_res.is_err() {
                continue;
            }

            let client_state = client_state_res
                .unwrap()
                .into_inner()
                .identified_client_state;
            if client_state.is_none() || client_state.clone().unwrap().client_state.is_none() {
                error!(
                    "Got None response for {}/transfer client state!",
                    channel.channel_id.clone()
                );
                continue;
            }
            let client_state_any = client_state.unwrap().client_state.unwrap();

            // Check to see if this client state contains foreign_chain_id (e.g. "cavity-1")
            let client_state = decode_any::<ClientState>(client_state_any).unwrap();
            if client_state.chain_id == foreign_chain_id {
                info!("Discovered IBC Channel: {:?}", channel);
                return Ok(channel);
            }
        }
        delay_for(Duration::from_secs(5)).await;
    }
    Err(CosmosGrpcError::BadResponse("No such channel".to_string()))
}

// Retrieves just the ID of the channel connecting the chain behind `ibc_channel_qc` and the chain with id `foreign_chain_id`
// Retries up to `timeout` (or OPERATION_TIMEOUT if None), checking each channel's client state to find the foreign chain's id
pub async fn get_channel_id(
    ibc_channel_qc: IbcChannelQueryClient<Channel>, // The Src chain's IbcChannelQueryClient
    foreign_chain_id: String,                       // The chain-id of the Dst chain
    timeout: Option<Duration>,
) -> Result<String, CosmosGrpcError> {
    Ok(get_channel(ibc_channel_qc, foreign_chain_id, timeout)
        .await?
        .channel_id)
}

// Sends an IBC transfer to move `coin` from sender on the `contact`'s chain over IBC to `receiver` on the
// channel that `channel_id` points to. `packet_timeout` is a number of seconds to give IBC to perform the transfer
// before it will fail.
pub async fn send_ibc_transfer(
    contact: &Contact,        // Src chain's deep_space client
    sender: impl PrivateKey,  // The Src chain's funds sender
    receiver: Address,        // The Dst chain's funds receiver
    coin: Coin,               // The coin to send to receiver
    fee_coin: Option<Coin>,   // The fee to pay for submitting the transfer msg
    channel_id: String,       // The Src chain's ibc channel connecting to Dst
    packet_timeout: Duration, // Used to create ibc-transfer timeout-timestamp
) -> Result<TxResponse, CosmosGrpcError> {
    let sender_address = sender
        .to_address(&contact.get_prefix())
        .unwrap()
        .to_string();

    let timeout_timestamp = SystemTime::now()
        .add(packet_timeout)
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let msg_transfer = MsgTransfer {
        source_port: "transfer".to_string(),
        source_channel: channel_id,
        token: Some(coin.clone().into()),
        sender: sender_address,
        receiver: receiver.to_string(),
        timeout_height: None,
        timeout_timestamp, // 150 minutes from now
        ..Default::default()
    };
    info!("Submitting MsgTransfer {:?}", msg_transfer);
    let msg_transfer = Msg::new(MSG_TRANSFER_TYPE_URL, msg_transfer);
    let fee_coin = fee_coin.unwrap_or(Coin {
        amount: 100u16.into(),
        denom: (*STAKING_TOKEN).to_string(),
    });
    contact
        .send_message(
            &[msg_transfer],
            Some("Test Relaying".to_string()),
            &[fee_coin],
            Some(OPERATION_TIMEOUT),
            sender,
        )
        .await
}

#[allow(clippy::too_many_arguments)]
pub async fn send_and_assert_ibc_transfer(
    contact: &Contact, // Src chain's deep_space client
    src_ibc_transfer_qc: IbcTransferQueryClient<Channel>, // Src chain's GRPC ibc-transfer query client
    dst_bank_qc: BankQueryClient<Channel>,                // Dst chain's GRPC x/bank query client
    dst_ibc_transfer_qc: IbcTransferQueryClient<Channel>, // Dst chain's GRPC ibc-transfer query client
    sender: impl PrivateKey,                              // The Src chain's funds sender
    receiver: Address,                                    // The Dst chain's funds receiver
    coin: Option<Coin>,                                   // The coin to send to receiver
    fee_coin: Option<Coin>,   // The fee to pay for submitting the transfer msg
    channel_id: String,       // The Src chain's ibc channel connecting to Dst
    packet_timeout: Duration, // Used to create ibc-transfer timeout-timestamp
) -> bool {
    let sender_address = sender
        .to_address(&contact.get_prefix())
        .unwrap()
        .to_string();

    let timeout_timestamp = SystemTime::now()
        .add(packet_timeout)
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let coin = coin.unwrap_or(Coin {
        denom: STAKING_TOKEN.to_string(),
        amount: one_atom(),
    });
    let pre_bal = get_ibc_balance(
        receiver,
        coin.denom.clone(),
        None,
        src_ibc_transfer_qc.clone(),
        dst_bank_qc.clone(),
        dst_ibc_transfer_qc.clone(),
    )
    .await;
    let msg_transfer = MsgTransfer {
        source_port: "transfer".to_string(),
        source_channel: channel_id,
        token: Some(coin.clone().into()),
        sender: sender_address,
        receiver: receiver.to_string(),
        timeout_height: None,
        timeout_timestamp, // 150 minutes from now
        ..Default::default()
    };
    info!("Submitting MsgTransfer {:?}", msg_transfer);
    let msg_transfer = Msg::new(MSG_TRANSFER_TYPE_URL, msg_transfer);
    let fee_coin = fee_coin.unwrap_or(Coin {
        amount: 100u16.into(),
        denom: (*STAKING_TOKEN).to_string(),
    });
    let send_res = contact
        .send_message(
            &[msg_transfer],
            Some("Test Relaying".to_string()),
            &[fee_coin],
            Some(OPERATION_TIMEOUT),
            sender,
        )
        .await;
    info!("Sent MsgTransfer with response {:?}", send_res);

    // Give the ibc-relayer a bit of time to work in the event of multiple runs
    delay_for(packet_timeout).await;

    let post_bal = get_ibc_balance(
        receiver,
        coin.denom.clone(),
        None,
        src_ibc_transfer_qc,
        dst_bank_qc,
        dst_ibc_transfer_qc,
    )
    .await;
    info!("IBC Transfer - Asserting balance change {pre_bal:?} -> {post_bal:?}");
    match (pre_bal, post_bal) {
        (None, None) => {
            error!(
                "Failed to transfer {} over ibc to {}!",
                coin.denom, receiver
            );
            return false;
        }
        (None, Some(post)) => {
            if post.amount != coin.amount {
                error!(
                    "Incorrect ibc balance for user {}: actual {:?} != expected {}",
                    receiver, post, coin.amount,
                );
                return false;
            }
            info!(
                "Successfully transfered {:?} (aka {}) over ibc!",
                coin, post.denom
            );
        }
        (Some(pre), Some(post)) => {
            let amount_uint = coin.amount;
            let pre_amt = pre.amount;
            let post_amt = post.amount;
            if post_amt < pre_amt || post_amt - pre_amt != amount_uint {
                error!(
                    "Incorrect ibc balance for user {}: actual {:?} != expected {}",
                    receiver,
                    post,
                    (pre_amt + amount_uint),
                );
                return false;
            }
            info!(
                "Successfully transfered {:?} (aka {}) over ibc!",
                coin, post.denom
            );
        }
        (Some(_), None) => {
            error!(
                "User wound up with no balance after ibc transfer? {}",
                receiver,
            );
            return false;
        }
    }
    true
}

// Retrieves the balance `account` holds of `src_denom`'s IBC representation
// Note: The Coin returned has the ibc/<HASH> denom, not the `src_chain` denom
// Retries up to `timeout` or OPERATION_TIMEOUT if not provided
#[allow(clippy::too_many_arguments)]
pub async fn get_ibc_balance(
    account: Address,               // The account's balance to check on dst
    src_denom: String,              // The name of the asset on Src chain
    dst_channel_id: Option<String>, // The name of the channel on Dst chain connecting to Src chain
    src_ibc_transfer_qc: IbcTransferQueryClient<Channel>, // Src chain's ibc-transfer GRPC client
    dst_bank_qc: BankQueryClient<Channel>, // Dst chain's Bank GRPC client
    dst_ibc_transfer_qc: IbcTransferQueryClient<Channel>, // Dst chain's ibc-transfer GRPC client
) -> Option<Coin> {
    let mut dst_bank_qc = dst_bank_qc;
    let dst_denom: String = if src_denom.starts_with("ibc/") {
        get_denom_trace_for_hash(src_denom, src_ibc_transfer_qc)
            .await
            .expect("Could not get denom trace for src_denom")
            .base_denom
    } else {
        let channel_id = dst_channel_id.unwrap_or("channel-0".to_string());
        let trace_res =
            get_hash_for_denom_trace(src_denom, channel_id, None, dst_ibc_transfer_qc).await;
        if trace_res.is_err() {
            return None;
        }
        trace_res.unwrap()
    };
    let res = dst_bank_qc
        .balance(QueryBalanceRequest {
            denom: dst_denom.clone(),
            address: account.to_string(),
        })
        .await;
    res.expect("No response from bank balance query?")
        .into_inner()
        .balance
        .map(|r| r.into())
}

pub async fn get_denom_trace_for_hash(
    mut hash: String,
    mut ibc_transfer_qc: IbcTransferQueryClient<Channel>,
) -> Result<DenomTrace, CosmosGrpcError> {
    if let Some(hsh) = hash.strip_prefix("ibc/") {
        hash = hsh.to_string();
    }

    ibc_transfer_qc
        .denom_trace(QueryDenomTraceRequest { hash })
        .await
        .map(|r| r.into_inner().denom_trace.expect("No denom trace found"))
        .map_err(|e| CosmosGrpcError::BadResponse(e.to_string()))
}

pub async fn get_hash_for_denom_trace(
    denom: String,
    channel_id: String,
    port_id: Option<String>,
    mut ibc_transfer_qc: IbcTransferQueryClient<Channel>,
) -> Result<String, CosmosGrpcError> {
    let port_id = port_id.unwrap_or("transfer".to_string());
    let trace = format!("{port_id}/{channel_id}/{denom}");
    let start = Instant::now();
    let mut res: Result<String, CosmosGrpcError> = Ok("Bad String".to_string());
    while Instant::now() - start < OPERATION_TIMEOUT {
        res = ibc_transfer_qc
            .denom_hash(QueryDenomHashRequest {
                trace: trace.clone(),
            })
            .await
            .map(|r| format!("ibc/{}", r.into_inner().hash))
            .map_err(|e| CosmosGrpcError::BadResponse(e.to_string()));
        if res.is_err() {
            delay_for(Duration::from_secs(5)).await;
            continue;
        }
    }

    res
}
