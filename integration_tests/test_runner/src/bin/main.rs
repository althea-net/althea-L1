//! This crate runs the tests defined in the src/tests library crate. The bin and lib are separate
//! so that upgrades can be tested via a git dependency, using the old tag
//! as the commit and the current code as the new version.
#[macro_use]
extern crate log;

use deep_space::Coin;
use deep_space::Contact;
use deep_space::PrivateKey;
use std::env;
use test_runner::bootstrapping::parse_contract_addresses;
use test_runner::bootstrapping::parse_ibc_validator_keys;
use test_runner::bootstrapping::send_erc20s_to_evm_users;
use test_runner::bootstrapping::start_ibc_relayer;
use test_runner::bootstrapping::{deploy_contracts, get_keys};
use test_runner::tests::erc20_conversion::erc20_conversion_test;
use test_runner::tests::ica_host::ica_host_happy_path;
use test_runner::tests::liquid_accounts::liquid_accounts_test;
use test_runner::tests::lockup::lockup_test;
use test_runner::tests::microtx_fees::microtx_fees_test;
use test_runner::tests::native_token::native_token_test;
use test_runner::tests::onboarding::onboarding_default_params;
use test_runner::tests::onboarding::onboarding_delist_after;
use test_runner::tests::onboarding::onboarding_disable_after;
use test_runner::tests::onboarding::onboarding_disabled_whitelisted;
use test_runner::utils::one_atom;
use test_runner::utils::one_hundred_eth;
use test_runner::utils::send_funds_bulk;
use test_runner::utils::ETH_NODE;
use test_runner::utils::EVM_USER_KEYS;
use test_runner::utils::IBC_ADDRESS_PREFIX;
use test_runner::utils::IBC_NODE_GRPC;
use test_runner::utils::STAKING_TOKEN;
use test_runner::utils::{
    get_test_token_name, should_deploy_contracts, wait_for_cosmos_online, ADDRESS_PREFIX,
    COSMOS_NODE_GRPC, OPERATION_TIMEOUT, TOTAL_TIMEOUT,
};

#[actix_rt::main]
pub async fn main() {
    env_logger::init();
    info!("Starting Althea test-runner");
    let contact = Contact::new(
        COSMOS_NODE_GRPC.as_str(),
        OPERATION_TIMEOUT,
        ADDRESS_PREFIX.as_str(),
    )
    .unwrap();
    let ibc_contact = Contact::new(
        IBC_NODE_GRPC.as_str(),
        OPERATION_TIMEOUT,
        IBC_ADDRESS_PREFIX.as_str(),
    )
    .unwrap();
    let web30 = web30::client::Web3::new(ETH_NODE.as_str(), OPERATION_TIMEOUT);

    if should_deploy_contracts() {
        info!("test-runner in contract deploying mode, deploying contracts, then exiting");
        deploy_contracts(&contact).await;
        return;
    }

    let contracts = parse_contract_addresses();
    // addresses of deployed ERC20 token contracts to be used for testing
    let erc20_addresses = contracts.erc20_addresses.clone();

    info!("Funding EVM users with ERC20s ({erc20_addresses:?})");
    send_erc20s_to_evm_users(
        &web30,
        erc20_addresses.clone(),
        EVM_USER_KEYS.clone(),
        one_hundred_eth(),
    )
    .await
    .unwrap();

    info!("Waiting for Cosmos chain to come online");
    wait_for_cosmos_online(&contact, TOTAL_TIMEOUT).await;
    info!("Cosmos chain is online!");

    // keys for the primary test chain
    let keys = get_keys();
    let (ibc_keys, _ibc_phrases) = parse_ibc_validator_keys();

    info!("Funding EVM users with the native coin");
    // Send the EVM users some althea token
    send_funds_bulk(
        &contact,
        keys.first().expect("No validator keys?").validator_key,
        &EVM_USER_KEYS
            .clone()
            .into_iter()
            .map(|euk| euk.ethermint_address)
            .collect::<Vec<_>>(),
        Coin {
            amount: one_atom(),
            denom: STAKING_TOKEN.to_string(),
        },
        Some(OPERATION_TIMEOUT),
    )
    .await
    .unwrap();

    info!("Checking footoken balances");
    // assert that the validators have a balance of the footoken we use
    // for test transfers
    assert!(contact
        .get_balance(
            keys[0]
                .validator_key
                .to_address(&contact.get_prefix())
                .unwrap(),
            get_test_token_name(),
        )
        .await
        .unwrap()
        .is_some());

    let test_type = env::var("TEST_TYPE");

    info!("Starting tests with {:?}", test_type);
    if let Ok(test_type) = test_type {
        if test_type == "LOCKUP" {
            info!("Starting Lockup test");
            lockup_test(&contact, keys).await;
            return;
        } else if test_type == "MICROTX_FEES" {
            info!("Starting microtx fees test");
            microtx_fees_test(&contact, keys).await;
            return;
        } else if test_type == "ERC20_CONVERSION" {
            info!("Starting erc20 conversion test");
            erc20_conversion_test(
                &contact,
                &web30,
                keys,
                EVM_USER_KEYS.clone(),
                erc20_addresses,
            )
            .await;
            return;
        } else if test_type == "NATIVE_TOKEN" {
            info!("Starting native token test");
            native_token_test(&contact, &web30, keys).await;
            return;
        } else if test_type == "LIQUID_ACCOUNTS" {
            info!("Start Liquid Infrastructure Accounts test");
            liquid_accounts_test(
                &contact,
                &web30,
                keys,
                erc20_addresses,
                EVM_USER_KEYS.clone(),
            )
            .await;
            return;
        } else if test_type == "ICA_HOST" {
            start_ibc_relayer(&contact, &ibc_contact, &keys, &ibc_keys).await;
            info!("Start ICA Host test");
            ica_host_happy_path(&contact, &ibc_contact, keys, ibc_keys).await;
            return;
        } else if test_type == "ONBOARDING_DEFAULT_PARAMS" {
            start_ibc_relayer(&contact, &ibc_contact, &keys, &ibc_keys).await;
            info!("Start onboarding default params test");
            onboarding_default_params(
                &contact,
                &ibc_contact,
                keys,
                ibc_keys,
                erc20_addresses,
                EVM_USER_KEYS.clone(),
            )
            .await;
            return;
        } else if test_type == "ONBOARDING_DISABLED_WHITELISTED" {
            start_ibc_relayer(&contact, &ibc_contact, &keys, &ibc_keys).await;
            info!("Start onboarding disabled yet whitelisted test");
            onboarding_disabled_whitelisted(
                &contact,
                &ibc_contact,
                keys,
                ibc_keys,
                erc20_addresses,
                EVM_USER_KEYS.clone(),
            )
            .await;
            return;
        } else if test_type == "ONBOARDING_DISABLE_AFTER" {
            start_ibc_relayer(&contact, &ibc_contact, &keys, &ibc_keys).await;
            info!("Start onboarding disable after test");
            onboarding_disable_after(
                &contact,
                &ibc_contact,
                keys,
                ibc_keys,
                erc20_addresses,
                EVM_USER_KEYS.clone(),
            )
            .await;
            return;
        } else if test_type == "ONBOARDING_DELIST_AFTER" {
            start_ibc_relayer(&contact, &ibc_contact, &keys, &ibc_keys).await;
            info!("Start onboarding delist after test");
            onboarding_delist_after(
                &contact,
                &ibc_contact,
                keys,
                ibc_keys,
                erc20_addresses,
                EVM_USER_KEYS.clone(),
            )
            .await;
            return;
        }
    }

    // this checks that the chain is continuing at the end of each test.
    contact
        .wait_for_next_block(TOTAL_TIMEOUT)
        .await
        .expect("Error chain has halted unexpectedly!");
}
