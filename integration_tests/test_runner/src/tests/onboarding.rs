use std::thread::sleep;
use std::time::Duration;

use althea_proto::althea::onboarding::v1::query_client::QueryClient as OnboardingQueryClient;
use althea_proto::althea::onboarding::v1::QueryParamsRequest;
use althea_proto::canto::erc20::v1::{
    query_client::QueryClient as Erc20QueryClient, RegisterCoinProposal, RegisterErc20Proposal,
};
use althea_proto::canto::erc20::v1::{MsgConvertErc20, QueryTokenPairRequest, TokenPair};
use althea_proto::cosmos_sdk_proto::cosmos::bank::v1beta1::query_client::QueryClient as BankQueryClient;
use althea_proto::cosmos_sdk_proto::cosmos::bank::v1beta1::{DenomUnit, Metadata};
use althea_proto::cosmos_sdk_proto::cosmos::base::abci::v1beta1::TxResponse;
use althea_proto::cosmos_sdk_proto::cosmos::params::v1beta1::{
    ParamChange, ParameterChangeProposal,
};
use althea_proto::cosmos_sdk_proto::ibc::applications::transfer::v1::query_client::QueryClient as IbcTransferQueryClient;
use althea_proto::cosmos_sdk_proto::ibc::core::channel::v1::query_client::QueryClient as IbcChannelQueryClient;
use althea_proto::cosmos_sdk_proto::ibc::core::channel::v1::IdentifiedChannel;
use clarity::{Address as EthAddress, Uint256};
use deep_space::address::cosmos_address_to_eth_address;
use deep_space::error::CosmosGrpcError;
use deep_space::utils::encode_any;
use deep_space::{Address, Coin, Contact, CosmosPrivateKey, EthermintPrivateKey, Msg, PrivateKey};
use num::Zero;
use prost_types::Any;
use tonic::transport::Channel;
use web30::client::Web3;

use crate::ibc_utils::{
    get_denom_trace_for_hash, get_hash_for_denom_trace, get_ibc_balance,
    send_and_assert_ibc_transfer, send_ibc_transfer,
};
use crate::type_urls::MSG_CONVERT_ERC20_TYPE_URL;
use crate::utils::{
    get_chain_id, one_atom, one_eth, one_hundred_atom, one_hundred_eth, send_funds_bulk,
    vote_yes_on_proposals, wait_for_proposals_to_execute, EthermintUserKey, ValidatorKeys,
    ADDRESS_PREFIX, ETH_NODE, IBC_ADDRESS_PREFIX, IBC_NODE_GRPC, IBC_STAKING_TOKEN,
    OPERATION_TIMEOUT, STAKING_TOKEN,
};
use crate::{
    ibc_utils::get_channel,
    utils::{get_ibc_chain_id, COSMOS_NODE_GRPC},
};

pub const MSG_REGISTER_ACCOUNT_TYPE_URL: &str = "/gaia.icaauth.v1.MsgRegisterAccount";
pub const MSG_SUBMIT_TX_TYPE_URL: &str = "/gaia.icaauth.v1.MsgSubmitTx";

/* Test cases:
 * 1. Onboarding module is not configured (default params)
 * 2. Onboarding module is configured but disabled
 * 3. Onboarding module is configured and enabled
 *   a. Perform a MsgSend and a MsgMicrotx
 * 4. Onboarding module was configured an enabled, but becomes disabled
 * 5. Onboarding module was configured and enabled, but the channel loses whitelist status
 */

// Tests that onboarding does nothing when enabled but no whitelist is configured
pub async fn onboarding_default_params(
    althea_contact: &Contact,
    ibc_contact: &Contact,
    keys: Vec<ValidatorKeys>,
    ibc_keys: Vec<CosmosPrivateKey>,
    erc20_contracts: Vec<EthAddress>,
    evm_user_keys: Vec<EthermintUserKey>,
) {
    onboarding_test(
        althea_contact,
        ibc_contact,
        keys,
        ibc_keys,
        OnboardingConfig {
            enable_onboarding: true,
            whitelist_channel: false,
            ..Default::default()
        },
        erc20_contracts,
        evm_user_keys,
    )
    .await;
}

// Tests that onboarding does nothing when disabled and the whitelist is configured
pub async fn onboarding_disabled_whitelisted(
    althea_contact: &Contact,
    ibc_contact: &Contact,
    keys: Vec<ValidatorKeys>,
    ibc_keys: Vec<CosmosPrivateKey>,
    erc20_contracts: Vec<EthAddress>,
    evm_user_keys: Vec<EthermintUserKey>,
) {
    onboarding_test(
        althea_contact,
        ibc_contact,
        keys,
        ibc_keys,
        OnboardingConfig {
            enable_onboarding: false,
            whitelist_channel: true,
            ..Default::default()
        },
        erc20_contracts,
        evm_user_keys,
    )
    .await;
}

// Tests that onboarding works correctly and then if disabled onboarding no longer applies
pub async fn onboarding_disable_after(
    althea_contact: &Contact,
    ibc_contact: &Contact,
    keys: Vec<ValidatorKeys>,
    ibc_keys: Vec<CosmosPrivateKey>,
    erc20_contracts: Vec<EthAddress>,
    evm_user_keys: Vec<EthermintUserKey>,
) {
    onboarding_test(
        althea_contact,
        ibc_contact,
        keys,
        ibc_keys,
        OnboardingConfig {
            enable_onboarding: true,
            whitelist_channel: true,
            disable_after: true,
            ..Default::default()
        },
        erc20_contracts,
        evm_user_keys,
    )
    .await;
}

// Tests that onboarding works correctly and then if the channel loses whitelist onboarding no longer applies
pub async fn onboarding_delist_after(
    althea_contact: &Contact,
    ibc_contact: &Contact,
    keys: Vec<ValidatorKeys>,
    ibc_keys: Vec<CosmosPrivateKey>,
    erc20_contracts: Vec<EthAddress>,
    evm_user_keys: Vec<EthermintUserKey>,
) {
    onboarding_test(
        althea_contact,
        ibc_contact,
        keys,
        ibc_keys,
        OnboardingConfig {
            enable_onboarding: true,
            whitelist_channel: true,
            delist_after: true,
            ..Default::default()
        },
        erc20_contracts,
        evm_user_keys,
    )
    .await;
}

#[derive(Debug, Clone, Default)]
pub struct OnboardingConfig {
    enable_onboarding: bool,
    whitelist_channel: bool,
    disable_after: bool,
    delist_after: bool,
}

pub async fn onboarding_test(
    althea_contact: &Contact,
    ibc_contact: &Contact,
    keys: Vec<ValidatorKeys>,
    ibc_keys: Vec<CosmosPrivateKey>,
    config: OnboardingConfig,
    erc20_contracts: Vec<EthAddress>,
    evm_user_keys: Vec<EthermintUserKey>,
) {
    let web30 = Web3::new(&ETH_NODE, OPERATION_TIMEOUT);
    let onboarding_qc = OnboardingQueryClient::connect(COSMOS_NODE_GRPC.as_str())
        .await
        .expect("Could not connect onboarding query client");
    let althea_channel_qc = IbcChannelQueryClient::connect(COSMOS_NODE_GRPC.as_str())
        .await
        .expect("Could not connect channel query client");

    // Wait for the ibc channel to be created and find the channel ids
    info!("Waiting for IBC channel creation:");
    let channel_id_timeout = Duration::from_secs(60 * 5);
    let althea_channel = get_channel(
        althea_channel_qc,
        get_ibc_chain_id(),
        Some(channel_id_timeout),
    )
    .await
    .expect("Could not find ibc-test-1 <-> althea_417834-1 channel");

    let TestSetupResults {
        erc20_denom_on_ibc,
        erc20_holder,
        erc20_token_pair,
        ibc_holder,
        ibc_token_pair,
    } = setup_onboarding(
        althea_contact,
        ibc_contact,
        onboarding_qc.clone(),
        &keys,
        &ibc_keys,
        althea_channel.clone(),
        config.clone(),
        erc20_contracts,
        evm_user_keys,
    )
    .await;

    let OnboardingConfig {
        enable_onboarding,
        whitelist_channel,
        disable_after,
        delist_after: un_whitelist_after,
    } = config;

    // Define the users and collect their pre-test balances on the EVM and Bank modules
    let erc20_holder_on_althea = erc20_holder.to_address(&ADDRESS_PREFIX).unwrap();
    let erc20_holder_on_evm = cosmos_address_to_eth_address(erc20_holder_on_althea).unwrap();

    let first_snapshot = get_balance_snapshot(
        althea_contact,
        &web30,
        erc20_holder_on_althea,
        erc20_holder_on_evm,
        erc20_token_pair.clone(),
        ibc_token_pair.clone(),
    )
    .await
    .expect("Could not get first balance snapshot");

    // Now to actually perform the test, sending IBC transfers to Althea-L1 and checking the balance changes
    // that happen in the bank module and on the erc20 contracts
    let first_erc20_transfer_amount = one_eth();
    send_erc20_back_to_althea(
        ibc_contact,
        ibc_holder,
        erc20_holder_on_althea,
        erc20_denom_on_ibc.denom.clone(),
        first_erc20_transfer_amount,
    )
    .await;
    let first_ibc_transfer_amount = one_atom();
    send_stake_to_althea(
        ibc_contact,
        ibc_holder,
        erc20_holder_on_althea,
        first_ibc_transfer_amount,
    )
    .await;

    let second_snapshot = get_balance_snapshot(
        althea_contact,
        &web30,
        erc20_holder_on_althea,
        erc20_holder_on_evm,
        erc20_token_pair.clone(),
        ibc_token_pair.clone(),
    )
    .await
    .expect("Could not get first balance snapshot");

    // We don't want to see any changes happen to the erc20s if enable_onboarding is false or if whitelist channel is false
    if !enable_onboarding || !whitelist_channel {
        snapshot_increase_on_bank_only(
            first_snapshot,
            second_snapshot.clone(),
            first_erc20_transfer_amount,
            first_ibc_transfer_amount,
        );
    } else {
        snapshot_increase_on_evm_only(
            first_snapshot,
            second_snapshot.clone(),
            first_erc20_transfer_amount,
            first_ibc_transfer_amount,
        )
    }

    if disable_after {
        // If disable_after we want to set another proposal which disables onboarding
        submit_and_pass_onboarding_proposal(
            althea_contact,
            onboarding_qc,
            &keys,
            althea_channel.channel_id.clone(),
            false,
            whitelist_channel,
        )
        .await;
        // after which we do not want to see ERC20 contract changes.
    } else if un_whitelist_after {
        // If un_whitelist_after we want to set another proposal which removes the channel from the whitelist
        submit_and_pass_onboarding_proposal(
            althea_contact,
            onboarding_qc,
            &keys,
            althea_channel.channel_id.clone(),
            enable_onboarding,
            false,
        )
        .await;
        // after which we also do not want to see ERC20 contract changes.
    } else {
        info!("Successfully tested onboarding module");
        return;
    }
    let second_erc20_transfer_amount = one_eth() * 10u8.into();
    send_erc20_back_to_althea(
        ibc_contact,
        ibc_holder,
        erc20_holder_on_althea,
        erc20_denom_on_ibc.denom.clone(),
        second_erc20_transfer_amount,
    )
    .await;
    let second_ibc_transfer_amount = one_atom() * 15u8.into();
    send_stake_to_althea(
        ibc_contact,
        ibc_holder,
        erc20_holder_on_althea,
        second_ibc_transfer_amount,
    )
    .await;

    let third_snapshot = get_balance_snapshot(
        althea_contact,
        &web30,
        erc20_holder_on_althea,
        erc20_holder_on_evm,
        erc20_token_pair.clone(),
        ibc_token_pair.clone(),
    )
    .await
    .expect("Could not get first balance snapshot");
    snapshot_increase_on_evm_only(
        second_snapshot,
        third_snapshot,
        second_erc20_transfer_amount,
        second_ibc_transfer_amount,
    );
    info!("Successsfully tested onboarding module");
}

// ---------------------------------------HELPER FUNCTIONS---------------------------------------

pub struct TestSetupResults {
    pub ibc_holder: CosmosPrivateKey,
    pub erc20_holder: EthermintPrivateKey,
    pub ibc_token_pair: TokenPair,
    pub erc20_token_pair: TokenPair,
    pub erc20_denom_on_ibc: Coin,
}

// Sets up an onboarding test, transferring tokens from either chain to the other over IBC,
// registering Coins for ERC20s and vice versa,
// and configuring the onboarding module
#[allow(clippy::too_many_arguments)]
async fn setup_onboarding(
    althea_contact: &Contact, // Althea-L1's contact
    ibc_contact: &Contact,    // Ibc test chain's contact
    onboarding_qc: OnboardingQueryClient<tonic::transport::Channel>,
    keys: &[ValidatorKeys],
    ibc_keys: &[CosmosPrivateKey],
    channel: IdentifiedChannel,
    OnboardingConfig {
        enable_onboarding,
        whitelist_channel,
        ..
    }: OnboardingConfig,
    erc20_contracts: Vec<EthAddress>,
    evm_user_keys: Vec<EthermintUserKey>,
) -> TestSetupResults {
    let ibc_account = ibc_keys.first().unwrap();
    let ibc_account_addr = ibc_account.to_address(&IBC_ADDRESS_PREFIX).unwrap();
    let erc20_holder = evm_user_keys.first().unwrap().ethermint_key;
    let erc20_holder_addr = erc20_holder
        .to_address(&ADDRESS_PREFIX)
        .expect("Could not get erc20 holder address");

    send_funds_bulk(
        althea_contact,
        keys.first().unwrap().validator_key,
        &[erc20_holder_addr],
        Coin {
            amount: one_eth(),
            denom: STAKING_TOKEN.to_string(),
        },
        Some(OPERATION_TIMEOUT),
    )
    .await
    .expect("Unable to fund ibc and erc20 holders with aalthea");

    // Register a Cosmos Coin for the ERC20 and an ERC20 for the IBC Stake token bridged to Althea-L1
    let mut althea_erc20_qc = Erc20QueryClient::connect(COSMOS_NODE_GRPC.as_str())
        .await
        .expect("Could not connect erc20 query client");

    let erc20 = erc20_contracts.first().unwrap();
    let erc20_address = erc20.to_string();
    submit_and_pass_token_proposal(
        althea_contact,
        althea_erc20_qc.clone(),
        keys,
        erc20_address.clone(),
        true,
    )
    .await;
    let erc20_pair = althea_erc20_qc
        .token_pair(QueryTokenPairRequest {
            token: erc20_address.clone(),
        })
        .await
        .expect("Could not get registered pair for erc20 token")
        .into_inner()
        .token_pair
        .expect("No pair for erc20 token after gov proposal");

    // Then we convert the ERC20 to a cosmos coin and bridge it to the IBC chain

    convert_erc20(
        althea_contact,
        erc20_holder,
        None,
        *erc20,
        one_hundred_eth(),
    )
    .await
    .expect("Unable to convert erc20 token");

    send_erc20_to_ibc_chain(
        althea_contact,
        erc20_holder,
        ibc_account_addr,
        erc20_pair.denom.clone(),
        one_eth() * 50u8.into(),
    )
    .await;
    let ibc_ibc_transfer_qc = IbcTransferQueryClient::connect(IBC_NODE_GRPC.as_str())
        .await
        .expect("Could not connect to ibc transfer query client");
    let althea_ibc_transfer_qc = IbcTransferQueryClient::connect(COSMOS_NODE_GRPC.as_str())
        .await
        .expect("Could not connect to ibc transfer query client");

    let ibc_erc20_denom = get_hash_for_denom_trace(
        erc20_pair.denom.clone(),
        "channel-0".to_string(),
        None,
        ibc_ibc_transfer_qc.clone(),
    )
    .await
    .expect("Could not get erc20 coin denom on ibc chain");
    let ibc_erc20_coin = ibc_contact
        .get_balance(ibc_account_addr, ibc_erc20_denom.clone())
        .await
        .expect("Could not get ibc erc20 balance")
        .unwrap_or(Coin {
            amount: 0u8.into(),
            denom: ibc_erc20_denom.clone(),
        });
    send_stake_to_althea(
        ibc_contact,
        *ibc_account,
        erc20_holder_addr,
        one_hundred_atom(),
    )
    .await;
    let ibc_stake_on_althea = get_hash_for_denom_trace(
        IBC_STAKING_TOKEN.to_string(),
        "channel-0".to_string(),
        None,
        althea_ibc_transfer_qc,
    )
    .await
    .expect("Could not get ibc stake denom on althea chain");
    submit_and_pass_token_proposal(
        althea_contact,
        althea_erc20_qc.clone(),
        keys,
        ibc_stake_on_althea.clone(),
        false,
    )
    .await;
    let ibc_stake_pair = althea_erc20_qc
        .token_pair(QueryTokenPairRequest {
            token: ibc_stake_on_althea.clone(),
        })
        .await
        .expect("Could not get registered pair for ibc stake token")
        .into_inner()
        .token_pair
        .expect("No pair for ibc stake token after gov proposal");
    // Finally, we configure the onboarding module for the test

    submit_and_pass_onboarding_proposal(
        althea_contact,
        onboarding_qc,
        keys,
        channel.channel_id,
        enable_onboarding,
        whitelist_channel,
    )
    .await;

    TestSetupResults {
        ibc_holder: *ibc_account,
        erc20_holder,
        ibc_token_pair: ibc_stake_pair,
        erc20_token_pair: erc20_pair,
        erc20_denom_on_ibc: ibc_erc20_coin,
    }
}

// Sends the ibc staking token to Althea-L1, returning the received amount on Althea-L1
pub async fn send_stake_to_althea(
    ibc_contact: &Contact, // Ibc test chain's contact
    ibc_sender: impl PrivateKey,
    althea_receiver: Address,
    amount: Uint256,
) -> Coin {
    let ibc_channel_qc = IbcChannelQueryClient::connect(IBC_NODE_GRPC.as_str())
        .await
        .expect("Could not connect channel query client");
    let ibc_to_althea_channel =
        get_channel(ibc_channel_qc, get_chain_id(), Some(OPERATION_TIMEOUT))
            .await
            .expect("Could not find ibc channel to Althea-L1");

    let ibc_coin = Coin {
        denom: IBC_STAKING_TOKEN.clone(),
        amount,
    };
    let zero_coin = Coin {
        denom: IBC_STAKING_TOKEN.clone(),
        amount: 0u8.into(),
    };
    send_ibc_transfer(
        ibc_contact,
        ibc_sender,
        althea_receiver,
        ibc_coin.clone(),
        Some(zero_coin),
        ibc_to_althea_channel.channel_id,
        Duration::from_secs(60),
    )
    .await
    .expect("Unable to send IBC Transfer");
    sleep(Duration::from_secs(60));

    ibc_coin
}

// Sends a wrapped ERC20 token to the IBC chain, returning the received amount on the IBC chain
pub async fn send_erc20_to_ibc_chain(
    althea_contact: &Contact, // Althea-L1's contact
    althea_sender: impl PrivateKey,
    ibc_receiver: Address,
    erc20_coin_denom: String, // The ERC20 as a Cosmos Coin
    amount: Uint256,
) -> Coin {
    let althea_channel_qc = IbcChannelQueryClient::connect(COSMOS_NODE_GRPC.as_str())
        .await
        .expect("Could not connect channel query client");
    let althea_to_ibc_channel = get_channel(
        althea_channel_qc,
        get_ibc_chain_id(),
        Some(OPERATION_TIMEOUT),
    )
    .await
    .expect("Could not find ibc channel to ibc chain");
    let ibc_coin = Coin {
        denom: erc20_coin_denom.clone(),
        amount,
    };
    let zero_coin = Coin {
        denom: STAKING_TOKEN.clone(),
        amount: 0u8.into(),
    };
    send_ibc_transfer(
        althea_contact,
        althea_sender,
        ibc_receiver,
        ibc_coin.clone(),
        Some(zero_coin),
        althea_to_ibc_channel.channel_id,
        Duration::from_secs(60),
    )
    .await
    .expect("Unable to send IBC Transfer");
    sleep(Duration::from_secs(60));

    ibc_coin
}

pub async fn send_erc20_back_to_althea(
    ibc_contact: &Contact, // Althea-L1's contact
    ibc_sender: impl PrivateKey,
    althea_receiver: Address,
    erc20_on_ibc_chain: String, // The ERC20 as an IBC denom on ibc-test-chain
    amount: Uint256,
) -> Coin {
    let ibc_channel_qc = IbcChannelQueryClient::connect(IBC_NODE_GRPC.as_str())
        .await
        .expect("Could not connect channel query client");
    let ibc_to_althea_channel =
        get_channel(ibc_channel_qc, get_chain_id(), Some(OPERATION_TIMEOUT))
            .await
            .expect("Could not find ibc channel to althea chain");
    let ibc_coin = Coin {
        denom: erc20_on_ibc_chain.clone(),
        amount,
    };
    let zero_coin = Coin {
        denom: IBC_STAKING_TOKEN.clone(),
        amount: 0u8.into(),
    };
    send_ibc_transfer(
        ibc_contact,
        ibc_sender,
        althea_receiver,
        ibc_coin.clone(),
        Some(zero_coin),
        ibc_to_althea_channel.channel_id,
        Duration::from_secs(60),
    )
    .await
    .expect("Unable to send IBC Transfer");
    sleep(Duration::from_secs(60));

    ibc_coin
}

// Converts an ERC20 (already registered as a token pair) to a Cosmos Coin, sending the Coin to the sender if no receiver is supplied
pub async fn convert_erc20(
    althea_contact: &Contact,
    sender: EthermintPrivateKey,
    receiver: Option<Address>,
    erc20: EthAddress,
    amount: Uint256,
) -> Result<TxResponse, CosmosGrpcError> {
    let sender_eth_addr =
        cosmos_address_to_eth_address(sender.to_address(ADDRESS_PREFIX.as_str()).unwrap())
            .expect("Unable to convert cosmos to eth address");
    let receiver =
        receiver.unwrap_or_else(|| sender.to_address(&althea_contact.get_prefix()).unwrap());
    let msg_convert_erc20 = MsgConvertErc20 {
        contract_address: erc20.to_string(),
        amount: amount.to_string(),
        sender: sender_eth_addr.to_string(),
        receiver: receiver.to_string(),
    };
    let msg = Msg::new(MSG_CONVERT_ERC20_TYPE_URL, msg_convert_erc20);
    althea_contact
        .send_message(&[msg], None, &[], Some(OPERATION_TIMEOUT), sender)
        .await
}

pub async fn submit_and_pass_onboarding_proposal(
    contact: &Contact, // Althea-L1's deep_space client
    onboarding_qc: OnboardingQueryClient<Channel>,
    keys: &[ValidatorKeys],
    channel_id: String,
    enable_onboarding: bool,
    whitelist_channel: bool,
) {
    let mut onboarding_qc = onboarding_qc;
    let onboarding_params = onboarding_qc
        .params(QueryParamsRequest {})
        .await
        .unwrap()
        .into_inner()
        .params
        .expect("No onboarding params returned?");
    let must_enable = enable_onboarding && !onboarding_params.enable_onboarding;
    let must_whitelist =
        whitelist_channel && !onboarding_params.whitelisted_channels.contains(&channel_id);
    if !must_enable && !must_whitelist {
        info!("Onboarding module already configured, skipping governance vote");
        return;
    } else {
        info!("Configuring onboarding via governance: enable={enable_onboarding}, whitelist={whitelist_channel}");
    }

    let deposit = Coin {
        amount: one_atom() * 100u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let fee = Coin {
        amount: 0u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let mut changes = Vec::new();
    changes.push(ParamChange {
        subspace: "onboarding".to_string(),
        key: "EnableOnboarding".to_string(),
        value: enable_onboarding.to_string(),
    });
    if whitelist_channel {
        changes.push(ParamChange {
            subspace: "onboarding".to_string(),
            key: "WhitelistedChannels".to_string(),
            value: serde_json::to_string(&[channel_id]).unwrap(),
        });
    } else {
        changes.push(ParamChange {
            subspace: "onboarding".to_string(),
            key: "WhitelistedChannels".to_string(),
            value: serde_json::to_string::<[String]>(&[]).unwrap(),
        })
    }

    let res = contact
        .submit_parameter_change_proposal(
            ParameterChangeProposal {
                title: "Configure Onboarding".to_string(),
                description: "Configure Onboarding".to_string(),
                changes,
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

pub async fn submit_and_pass_token_proposal(
    contact: &Contact, // Althea-L1's deep_space client
    erc20_qc: Erc20QueryClient<Channel>,
    keys: &[ValidatorKeys],
    token: String,
    is_erc20: bool,
) {
    let mut erc20_qc = erc20_qc;
    let maybe_token_pair: Option<TokenPair> = erc20_qc
        .token_pair(QueryTokenPairRequest {
            token: token.clone(),
        })
        .await
        .map_or(None, |r| r.into_inner().token_pair);
    if let Some(pair) = maybe_token_pair {
        info!("Pair already created for {token}: {pair:?}, skipping governance vote");
        return;
    }

    let deposit = Coin {
        amount: one_atom() * 100u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let fee = Coin {
        amount: 0u8.into(),
        denom: STAKING_TOKEN.clone(),
    };

    let res = create_token_proposal(
        contact,
        token,
        is_erc20,
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

// Creates either a RegisterERC20 proposal or a RegisterCoin proposal depending on is_erc20
pub async fn create_token_proposal(
    contact: &Contact, // Althea-L1's deep_space client
    denom: String,
    is_erc20: bool,
    deposit: Coin,
    fee: Coin,
    key: impl PrivateKey,
    wait_timeout: Option<Duration>,
) -> Result<TxResponse, CosmosGrpcError> {
    let proposal_any: Any = if is_erc20 {
        let proposal = RegisterErc20Proposal {
            title: format!("Create Cosmos Coin for {denom}"),
            description: format!("Create Cosmos Coin for {denom}"),
            erc20address: denom.clone(),
        };
        encode_any(
            proposal,
            deep_space::client::type_urls::REGISTER_ERC20_PROPOSAL_TYPE_URL.to_string(),
        )
    } else {
        let metadata = Metadata {
            base: denom.clone(),
            description: format!("IBC Token {denom}"),
            denom_units: vec![DenomUnit {
                aliases: vec![],
                denom: denom.clone(),
                exponent: 0,
            }],
            display: denom.clone(),
            name: denom.clone(),
            symbol: denom.clone(),
        };
        let proposal = RegisterCoinProposal {
            title: format!("Create Cosmos Coin for {denom}"),
            description: format!("Create Cosmos Coin for {denom}"),
            metadata: Some(metadata),
        };
        encode_any(
            proposal,
            deep_space::client::type_urls::REGISTER_COIN_PROPOSAL_TYPE_URL.to_string(),
        )
    };
    contact
        .create_gov_proposal(proposal_any, deposit, fee, key, wait_timeout)
        .await
}

#[derive(Debug, Clone)]
pub struct BalanceSnapshot {
    pub erc20_balance_bank: Uint256,
    pub ibc_balance_bank: Uint256,
    pub erc20_balance_evm: Uint256,
    pub ibc_balance_evm: Uint256,
}

// Fetches a snapshot of the balances of the erc20 and ibc tokens on the bank and erc20 contracts for the provided holders
#[allow(clippy::too_many_arguments)]
pub async fn get_balance_snapshot(
    althea_contact: &Contact,
    web30: &Web3,
    erc20_holder_cosmos: Address,
    erc20_holder_on_evm: EthAddress,
    erc20_token_pair: TokenPair,
    ibc_token_pair: TokenPair,
) -> Result<BalanceSnapshot, CosmosGrpcError> {
    let erc20_balance_bank = althea_contact
        .get_balance(erc20_holder_cosmos, erc20_token_pair.denom.clone())
        .await?
        .map_or(Uint256::zero(), |r| r.amount);
    let ibc_balance_bank = althea_contact
        .get_balance(erc20_holder_cosmos, ibc_token_pair.denom.clone())
        .await?
        .map_or(Uint256::zero(), |r| r.amount);
    let erc20_balance_evm = web30
        .get_erc20_balance(
            erc20_token_pair.erc20_address.parse().unwrap(),
            erc20_holder_on_evm,
        )
        .await
        .map_err(|e| CosmosGrpcError::BadResponse(e.to_string()))?;
    let ibc_balance_evm = web30
        .get_erc20_balance(
            ibc_token_pair.erc20_address.parse().unwrap(),
            erc20_holder_on_evm,
        )
        .await
        .map_err(|e| CosmosGrpcError::BadResponse(e.to_string()))?;

    Ok(BalanceSnapshot {
        erc20_balance_bank,
        ibc_balance_bank,
        erc20_balance_evm,
        ibc_balance_evm,
    })
}

// Verifies that the snapshot increase is only on the bank balances, i.e. no onboarding module changes happened
pub fn snapshot_increase_on_bank_only(
    first_snapshot: BalanceSnapshot,
    second_snapshot: BalanceSnapshot,
    erc20_transfer_amount: Uint256,
    ibc_transfer_amount: Uint256,
) {
    let erc20_bank_difference =
        second_snapshot.erc20_balance_bank - first_snapshot.erc20_balance_bank;
    assert_eq!(erc20_bank_difference, erc20_transfer_amount);
    let ibc_bank_difference = second_snapshot.ibc_balance_bank - first_snapshot.ibc_balance_bank;
    assert_eq!(ibc_bank_difference, ibc_transfer_amount);
    assert_eq!(
        second_snapshot.erc20_balance_evm,
        first_snapshot.erc20_balance_evm
    );
    assert_eq!(
        second_snapshot.ibc_balance_evm,
        first_snapshot.ibc_balance_evm
    );
}

// Verifies that the snapshot increase is only on the evm balances, i.e. onboarding module wrapped the incoming tokens as ERC20s
pub fn snapshot_increase_on_evm_only(
    first_snapshot: BalanceSnapshot,
    second_snapshot: BalanceSnapshot,
    erc20_transfer_amount: Uint256,
    ibc_transfer_amount: Uint256,
) {
    let erc20_evm_difference = second_snapshot.erc20_balance_evm - first_snapshot.erc20_balance_evm;
    assert_eq!(erc20_evm_difference, erc20_transfer_amount);
    let ibc_evm_difference = second_snapshot.ibc_balance_evm - first_snapshot.ibc_balance_evm;
    assert_eq!(ibc_evm_difference, ibc_transfer_amount);
    assert_eq!(
        second_snapshot.erc20_balance_bank,
        first_snapshot.erc20_balance_bank
    );
    assert_eq!(
        second_snapshot.ibc_balance_bank,
        first_snapshot.ibc_balance_bank
    );
}
