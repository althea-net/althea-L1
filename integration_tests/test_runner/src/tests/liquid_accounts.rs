use std::str::FromStr;

use crate::evm_utils::{
    get_liquid_account_owner, get_thresholds, set_thresholds, withdraw_liquid_account_balances,
    LiquidInfrastructureThreshold,
};
use crate::type_urls::MSG_LIQUIFY_TYPE_URL;
use crate::utils::{
    execute_register_coin_proposal, get_fee, get_test_token_name, get_unregistered_coin_for_erc20,
    get_user_key, one_eth, send_funds_bulk, EthermintUserKey, RegisterCoinProposalParams,
    ValidatorKeys, COINS_FOR_REGISTERING, MIN_GLOBAL_FEE_AMOUNT, OPERATION_TIMEOUT, STAKING_TOKEN,
    TOTAL_TIMEOUT,
};
use althea_proto::althea::microtx::v1::query_client::QueryClient as MicrotxQueryClient;
use althea_proto::althea::microtx::v1::{
    LiquidInfrastructureAccount, MsgLiquify, QueryLiquidAccountRequest,
};
use althea_proto::canto::erc20::v1::query_client::QueryClient as Erc20QueryClient;
use althea_proto::canto::erc20::v1::QueryTokenPairRequest;
use althea_proto::cosmos_sdk_proto::cosmos::bank::v1beta1::query_client::QueryClient as BankQueryClient;
use althea_proto::cosmos_sdk_proto::cosmos::bank::v1beta1::QueryDenomMetadataRequest;
use althea_proto::cosmos_sdk_proto::cosmos::base::abci::v1beta1::TxResponse;
use clarity::{Address as EthAddress, Uint256};
use deep_space::error::CosmosGrpcError;
use deep_space::{Address as CosmosAddress, Coin, Contact, Fee, Msg, PrivateKey};
use tonic::transport::Channel;
use web30::client::Web3;

pub const SKIP_GOV: bool = false;

/// Simulates activity of automated peer-to-peer transactions on Althea networks,
/// asserting that the correct fees are deducted and transfers succeed
pub async fn liquid_accounts_test(
    contact: &Contact,
    web3: &Web3,
    validator_keys: Vec<ValidatorKeys>,
    erc20s: Vec<EthAddress>,
    erc20_holders: Vec<EthermintUserKey>,
) {
    info!("Start liquid Accounts test");
    let mut bank_qc = BankQueryClient::connect(contact.get_url())
        .await
        .expect("Unable to create bank query client");
    let mut erc20_qc = Erc20QueryClient::connect(contact.get_url())
        .await
        .expect("Unable to create erc20 query client");
    let mut microtx_qc = MicrotxQueryClient::connect(contact.get_url())
        .await
        .expect("Unable to create microtx query client");
    let owner = get_user_key(None);
    let to_liquify = get_user_key(None);

    // Fund the owner and account to liquify
    let send_user = Coin {
        amount: one_eth() * 10u8.into(),
        denom: STAKING_TOKEN.to_string(),
    };
    send_funds_bulk(
        contact,
        validator_keys[0].validator_key,
        &[to_liquify.ethermint_address, owner.ethermint_address],
        send_user.clone(),
        Some(OPERATION_TIMEOUT),
    )
    .await
    .unwrap_or_else(|_| panic!("Could not fund test user {:?}", send_user));

    let liquify_res = liquify_account(
        contact,
        to_liquify.ethermint_key,
        to_liquify.ethermint_address,
        None,
    )
    .await
    .expect("Unable to liquify account");
    info!(
        "Got liquify account response:\n{}\nTx Hash: {}\nGas used: {}",
        liquify_res.raw_log, liquify_res.txhash, liquify_res.gas_used
    );

    let (liquid_account, _eth_owner) =
        assert_correct_account(web3, &mut microtx_qc, to_liquify.ethermint_address).await;
    let liquid_account_nft = EthAddress::from_str(&liquid_account.nft_address)
        .expect("Invalid NFT address returned from grpc");

    // Expect empty thresholds for the new account
    check_thresholds(web3, liquid_account_nft, to_liquify.eth_address, vec![]).await;

    // Configure Coin -> ERC20 proposals, wait for their execution
    let registered_coin = if !SKIP_GOV {
        let coin_erc20 = get_unregistered_coin_for_erc20(erc20_qc.clone()).await;
        let metadata = bank_qc
            .denom_metadata(QueryDenomMetadataRequest {
                denom: coin_erc20.clone(),
            })
            .await
            .expect("Unable to query denom metadata")
            .into_inner()
            .metadata
            .expect("No metadata for erc20 coin");
        let coin_params = RegisterCoinProposalParams {
            coin_metadata: metadata.clone(),
            proposal_desc: "Register Coin Proposal Description".to_string(),
            proposal_title: "Register Coin Proposal Title".to_string(),
        };
        execute_register_coin_proposal(contact, &validator_keys, Some(TOTAL_TIMEOUT), coin_params)
            .await;
        coin_erc20
    } else {
        COINS_FOR_REGISTERING.first().unwrap().clone()
    };
    let pair = erc20_qc
        .token_pair(QueryTokenPairRequest {
            token: registered_coin.clone(),
        })
        .await
        .expect("Unable to get registered ERC20")
        .into_inner()
        .token_pair
        .expect("found no registered coin");
    let (registered_coin, registered_erc20) = (
        pair.denom,
        EthAddress::from_str(&pair.erc20_address)
            .expect("got invalid erc20_address from query token pair"),
    );

    info!("Sending the ERC20 holders some of the Cosmos<>ERC20 coin");
    for k in &validator_keys {
        for h in &erc20_holders {
            contact
                .send_coins(
                    Coin {
                        amount: one_eth(),
                        denom: registered_coin.clone(),
                    },
                    Some(get_fee(None)),
                    h.ethermint_address,
                    Some(OPERATION_TIMEOUT),
                    k.validator_key,
                )
                .await
                .expect("Unable to send erc20 holder the registered coin");
        }
    }

    let threshold_limit = 100u8.into();
    let mut new_thresholds = vec![];
    for erc20 in erc20s {
        new_thresholds.push(LiquidInfrastructureThreshold {
            token: erc20,
            amount: threshold_limit,
        });
    }
    new_thresholds.push(LiquidInfrastructureThreshold {
        token: registered_erc20,
        amount: threshold_limit,
    });

    set_thresholds(
        web3,
        liquid_account_nft,
        to_liquify.eth_privkey,
        new_thresholds.clone(),
        Some(OPERATION_TIMEOUT),
    )
    .await
    .expect("Unable to set thresholds on liquid Account NFT");

    check_thresholds(
        web3,
        liquid_account_nft,
        to_liquify.eth_address,
        new_thresholds,
    )
    .await;

    // Create MsgMicrotxs with 0 balances, balances less than the trigger, balances at the trigger, and balances above the trigger
    // Expect total transfers over the threshold to leave only the threshold in the router account, then query the NFT's balance of associated tokens
    let redirected_balances = execute_microtxs(
        contact,
        web3,
        &validator_keys,
        &erc20_holders,
        liquid_account,
        registered_coin,
        registered_erc20,
        threshold_limit,
    )
    .await;
    info!("Successfully redirected balances from a liquid Account to the registered NFT");
    // TODO: Further testing should include multiple tokens, including native EVM tokens which have become cosmos coins.
    // Thresholds should also be updated to either expand or reduce the allowable balances, excess balances would be funneled on microtx

    // TODO: Try calling with tokens not held by the NFT and assert transaction revert
    // TODO: Try calling withdraw_liquid_account_balances_to as well, make sure those balances end up in the right location
    let withdraw_erc20s: Vec<EthAddress> = redirected_balances.iter().map(|b| b.0).collect();
    withdraw_liquid_account_balances(
        web3,
        liquid_account_nft,
        to_liquify.eth_privkey,
        withdraw_erc20s,
        None,
    )
    .await
    .expect("Unable to withdraw NFT balances");
    assert_erc20_balances(web3, redirected_balances, to_liquify.eth_address, None).await;
    info!("Successfully withdrew balances from a liquid Account to the registered NFT");

    // TODO: Change ownership of the NFT and assert the old owner has no authority, the new owner receives balances and can set thresholds
}

/// Submits a MsgLiquify which will liquify the `to_liquify` account
/// If fee is none, the minimum amount of the staking token will be used
pub async fn liquify_account(
    contact: &Contact,
    to_liquify_key: impl PrivateKey,
    to_liquify: CosmosAddress,
    fee: Option<Coin>,
) -> Result<TxResponse, CosmosGrpcError> {
    let fee = fee.unwrap_or_else(|| Coin {
        amount: MIN_GLOBAL_FEE_AMOUNT.into(),
        denom: STAKING_TOKEN.to_string(),
    });
    let liquify = MsgLiquify {
        sender: to_liquify.to_string(),
    };
    let msg = Msg::new(MSG_LIQUIFY_TYPE_URL, liquify);
    let msg_args = contact
        .get_message_args(
            to_liquify,
            Fee {
                amount: vec![fee],
                gas_limit: 2_500_000,
                payer: None,
                granter: None,
            },
        )
        .await?;
    contact
        .send_message_with_args(
            &[msg],
            None,
            msg_args,
            Some(OPERATION_TIMEOUT),
            to_liquify_key,
        )
        .await
}

// Checks the microtx module query response against the output from eth_call for correctness
pub async fn assert_correct_account(
    web3: &Web3,
    microtx_qc: &mut MicrotxQueryClient<Channel>,
    account_addr: CosmosAddress,
) -> (LiquidInfrastructureAccount, EthAddress) {
    let ta = microtx_qc
        .liquid_account(QueryLiquidAccountRequest {
            account: account_addr.to_string(),
            ..Default::default()
        })
        .await;
    info!("Got liquid account: {ta:?}");
    let tas = ta.unwrap().into_inner().accounts;
    assert_eq!(tas.len(), 1);
    let ta = tas.first().expect("No liquid account?");

    info!("Checking owner of account: {ta:?}");
    let query_owner = CosmosAddress::from_bech32(ta.owner.clone())
        .expect("Invalid owner address returned from liquid Account query");
    let reported_owner = deep_space::address::cosmos_address_to_eth_address(query_owner)
        .expect("Unable to convert owner to eip-55 address");
    let nft_address = EthAddress::from_str(&ta.nft_address)
        .expect("Invalid nft-address returned from liquid account query!");
    let owner = get_liquid_account_owner(web3, nft_address, Some(reported_owner))
        .await
        .expect("Unable to query owner of liquid Account");
    assert_eq!(owner, reported_owner);

    (ta.clone(), owner)
}

// Queries the thresholds on the NFT and asserts the response is as expected
pub async fn check_thresholds(
    web3: &Web3,
    nft_address: EthAddress,
    querier: EthAddress,
    expected_thresholds: Vec<LiquidInfrastructureThreshold>,
) {
    let thresholds = get_thresholds(web3, nft_address, Some(querier))
        .await
        .expect("Unable to get thresholds from nft");
    assert_eq!(thresholds.len(), expected_thresholds.len());
    for (a, b) in thresholds.into_iter().zip(expected_thresholds.into_iter()) {
        assert_eq!(a.amount, b.amount);
        assert_eq!(a.token, b.token);
    }
    info!("Thresholds set correctly");
}

// Executes and checks the result of many microtxs, trying to accrue a balance up to and past the threshold value set earlier
// Balances over the threshold must be sent to the nft, leaving precisely the threshold amount in the liquid account
// Received balances of tokens which are not convertible Cosmos<>EVM tokens should be rejected (we use footoken)
// Returns the list of balances which have been redirected to the NFT
#[allow(clippy::too_many_arguments)]
pub async fn execute_microtxs(
    contact: &Contact,
    web3: &Web3,
    keys: &[ValidatorKeys],
    erc20_holders: &[EthermintUserKey],
    liquid_account: LiquidInfrastructureAccount,
    registered_denom: String,
    registered_erc20: EthAddress,
    threshold_value: Uint256,
) -> Vec<(EthAddress, Uint256)> {
    let fee_coin = get_fee(None);
    let unregistered_coin = get_fee(Some(get_test_token_name()));
    let zero_coin = Coin {
        denom: registered_denom.clone(),
        amount: 0u8.into(),
    };
    let min_coin = Coin {
        denom: registered_denom.clone(),
        amount: 1u8.into(),
    };
    let threshold_coin = Coin {
        denom: registered_denom.clone(),
        amount: threshold_value,
    };
    let lt_threshold_coin = Coin {
        denom: registered_denom.clone(),
        amount: (threshold_value - 5u8.into()),
    };
    let destination = CosmosAddress::from_str(&liquid_account.account).unwrap();
    let nft_address = EthAddress::from_str(&liquid_account.nft_address).unwrap();
    let querier = erc20_holders.first().unwrap().eth_address;

    // Send unregistered coin
    let res = contact
        .send_microtx(
            unregistered_coin.clone(),
            Some(fee_coin.clone()),
            destination,
            Some(OPERATION_TIMEOUT),
            keys[0].validator_key,
        )
        .await;
    info!(
        "send_microtx unregistered coin response: {:?}",
        res.map(|r| r.raw_log)
    );

    expect_cosmos_evm_balances(
        contact,
        web3,
        CosmEvmBalanceChange {
            liquid_account: destination,
            cosmos_denom: unregistered_coin.denom,
            cosmos_amount: zero_coin.amount,
            nft_address,
            erc20_address: registered_erc20,
            erc20_amount: zero_coin.amount,
        },
        querier,
    )
    .await;

    // Send with amount = 0
    let res = contact
        .send_microtx(
            zero_coin.clone(),
            Some(fee_coin.clone()),
            destination,
            Some(OPERATION_TIMEOUT),
            erc20_holders[0].ethermint_key,
        )
        .await;
    info!(
        "send_microtx zero coin response: {:?}",
        res.map(|r| r.raw_log)
    );
    expect_cosmos_evm_balances(
        contact,
        web3,
        CosmEvmBalanceChange {
            liquid_account: destination,
            cosmos_denom: registered_denom.clone(),
            cosmos_amount: zero_coin.amount,
            nft_address,
            erc20_address: registered_erc20,
            erc20_amount: zero_coin.amount,
        },
        querier,
    )
    .await;

    // Send with amount = 1 (total 1)
    let res = contact
        .send_microtx(
            min_coin.clone(),
            Some(fee_coin.clone()),
            destination,
            Some(OPERATION_TIMEOUT),
            erc20_holders[0].ethermint_key,
        )
        .await;
    info!(
        "send_microtx min coin response: {:?}",
        res.map(|r| r.raw_log)
    );
    expect_cosmos_evm_balances(
        contact,
        web3,
        CosmEvmBalanceChange {
            liquid_account: destination,
            cosmos_denom: registered_denom.clone(),
            cosmos_amount: min_coin.amount,
            nft_address,
            erc20_address: registered_erc20,
            erc20_amount: zero_coin.amount,
        },
        querier,
    )
    .await;

    // Send with amount = 95 (total 96)
    let res = contact
        .send_microtx(
            lt_threshold_coin.clone(),
            Some(fee_coin.clone()),
            destination,
            Some(OPERATION_TIMEOUT),
            erc20_holders[1].ethermint_key,
        )
        .await;
    info!(
        "send_microtx under threshold coin response: {:?}",
        res.map(|r| r.raw_log)
    );
    expect_cosmos_evm_balances(
        contact,
        web3,
        CosmEvmBalanceChange {
            liquid_account: destination,
            cosmos_denom: registered_denom.clone(),
            cosmos_amount: 96u32.into(),
            nft_address,
            erc20_address: registered_erc20,
            erc20_amount: zero_coin.amount,
        },
        querier,
    )
    .await;

    // Send with amount = 100 (total 196)
    let res = contact
        .send_microtx(
            threshold_coin.clone(),
            Some(fee_coin.clone()),
            destination,
            Some(OPERATION_TIMEOUT),
            erc20_holders[1].ethermint_key,
        )
        .await;
    info!(
        "send_microtx eq threshold coin response: {:?}",
        res.map(|r| r.raw_log)
    );
    expect_cosmos_evm_balances(
        contact,
        web3,
        CosmEvmBalanceChange {
            liquid_account: destination,
            cosmos_denom: registered_denom.clone(),
            cosmos_amount: 100u32.into(),
            nft_address,
            erc20_address: registered_erc20,
            erc20_amount: 96u32.into(),
        },
        querier,
    )
    .await;

    // Return the confirmed balances held by the nft
    vec![(registered_erc20, 96u32.into())]
}

// A struct used to simplify parameters to `expect_cosmos_evm_balances()`
#[derive(Clone, Debug)]
struct CosmEvmBalanceChange {
    pub liquid_account: CosmosAddress,
    pub cosmos_denom: String,
    pub cosmos_amount: Uint256,
    pub nft_address: EthAddress,
    pub erc20_address: EthAddress,
    pub erc20_amount: Uint256,
}

async fn expect_cosmos_evm_balances(
    contact: &Contact,
    web3: &Web3,
    input: CosmEvmBalanceChange,
    querier: EthAddress,
) {
    let CosmEvmBalanceChange {
        liquid_account,
        cosmos_denom,
        cosmos_amount,
        nft_address,
        erc20_address,
        erc20_amount,
    } = input;
    let cosmos_actual = contact
        .get_balance(liquid_account, cosmos_denom.clone())
        .await
        .expect("Could not get cosmos balance of token");
    assert_eq!(
        cosmos_actual
            .unwrap_or_else(|| Coin {
                denom: cosmos_denom,
                amount: 0u8.into()
            })
            .amount,
        cosmos_amount
    );

    let evm_actual = web3
        .get_erc20_balance_as_address(Some(querier), erc20_address, nft_address)
        .await
        .expect("failed to query erc20 balance of nft contract");
    assert_eq!(evm_actual, erc20_amount);
}

async fn assert_erc20_balances(
    web3: &Web3,
    erc20s: Vec<(EthAddress, Uint256)>,
    wallet: EthAddress,
    querier: Option<EthAddress>,
) {
    let querier = querier.unwrap_or(wallet);
    for (erc20, expected_amt) in erc20s {
        let balance = web3
            .get_erc20_balance_as_address(Some(querier), erc20, wallet)
            .await
            .expect("Could not get erc20 balance");
        assert_eq!(expected_amt, balance);
    }
}
