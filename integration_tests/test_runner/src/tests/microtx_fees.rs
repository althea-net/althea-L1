use crate::type_urls::MSG_MICROTX_TYPE_URL;
use crate::utils::{
    bulk_get_user_keys, get_convertible_coin, one_atom_128, send_funds_bulk, EthermintUserKey,
    ValidatorKeys, ADDRESS_PREFIX, OPERATION_TIMEOUT, STAKING_TOKEN,
};
use althea_proto::althea::microtx::v1::MsgMicrotx;
use althea_proto::cosmos_sdk_proto::cosmos::base::v1beta1::Coin as ProtoCoin;
use clarity::Uint256;
use deep_space::{Address, Coin, Contact, Msg, PrivateKey};
use rand::distributions::Uniform;
use rand::{thread_rng, Rng};

pub const BASIS_POINTS_DIVISOR: u128 = 10_000;
pub const MICROTX_SUBSPACE: &str = "microtx";
/// This PARAM_KEY constant is defined in x/microtx/types/genesis.go and must match exactly
pub const MICROTX_FEE_BASIS_POINTS_PARAM_KEY: &str = "MicrotxFeeBasisPoints";

/// Simulates activity of automated peer-to-peer transactions on Althea networks,
/// asserting that the correct fees are deducted and transfers succeed
pub async fn microtx_fees_test(contact: &Contact, validator_keys: Vec<ValidatorKeys>) {
    let num_users = 64;
    // Make users who will send tokens
    let senders = bulk_get_user_keys(None, num_users);

    // Make users who will receive tokens via MsgMicrotx
    let receivers = bulk_get_user_keys(None, num_users);

    // Send one footoken to each sender
    let foo_balance = one_atom_128();
    let coin_denom = get_convertible_coin(contact, &validator_keys).await;
    let amount = Coin {
        amount: foo_balance.into(),
        denom: coin_denom.clone(),
    };
    send_funds_bulk(
        contact,
        validator_keys.first().unwrap().validator_key,
        &senders
            .clone()
            .iter()
            .map(|u| u.ethermint_address)
            .collect::<Vec<Address>>(),
        amount.clone(),
        Some(OPERATION_TIMEOUT),
    )
    .await
    .expect("Unable to send funds to all senders!");

    info!("Sending test microtx");
    let res = contact
        .send_microtx(
            amount,
            None,
            validator_keys[1]
                .validator_key
                .to_address(&ADDRESS_PREFIX)
                .unwrap(),
            None,
            validator_keys[0].validator_key,
        )
        .await;
    info!("Microtx res {res:?}");

    let param = contact
        .get_param(MICROTX_SUBSPACE, MICROTX_FEE_BASIS_POINTS_PARAM_KEY)
        .await
        .expect("Unable to get MicrotxFeeBasisPoints from microtx module");
    let param = param.param.unwrap().value;
    let microtx_fee_basis_points = param.trim_matches('"');
    info!(
        "Got microtx_fee_basis_points: [{}]",
        microtx_fee_basis_points
    );

    let microtx_fee_basis_points: u128 = serde_json::from_str(microtx_fee_basis_points).unwrap();
    let (microtxs, amounts, fees) = generate_msg_microtxs(
        &senders,
        &receivers,
        &coin_denom,
        foo_balance,
        microtx_fee_basis_points,
    );

    // Send the MsgMicrotxs, check their execution, assert the balances have changed
    exec_and_check(contact, &senders, &microtxs, &amounts, &fees, foo_balance).await;

    // Check that the senders and receivers have the expected balance
    assert_balance_changes(
        contact,
        &senders,
        &receivers,
        &amounts,
        &fees,
        foo_balance,
        &coin_denom,
    )
    .await;
}

/// Creates 3 Vec's: MsgMicrotx's, transfer amounts, and expected fees
/// The generated MsgMicrotx's will have a randomized transfer amount and a derived associated fee
/// Order is preserved so the i-th Msg corresponds to the i-th amount and the i-th fee
pub fn generate_msg_microtxs(
    senders: &[EthermintUserKey],
    receivers: &[EthermintUserKey],
    denom: &str,
    sender_balance: u128,
    microtx_fee_basis_points: u128,
) -> (Vec<Msg>, Vec<Uint256>, Vec<Uint256>) {
    let mut msgs = Vec::with_capacity(senders.len());
    let mut amounts = Vec::with_capacity(senders.len());
    let mut fees = Vec::with_capacity(senders.len());

    let mut rng = thread_rng();
    let amount_range = Uniform::new(0u128, sender_balance);
    for (i, (sender, receiver)) in senders.iter().zip(receivers.iter()).enumerate() {
        let amount: u128 = if i == 0 {
            // Guarantee one MsgMicrotx failure
            sender_balance
        } else {
            rng.sample(amount_range)
        };
        let expected_fee: u128 = amount * microtx_fee_basis_points / BASIS_POINTS_DIVISOR;
        let amount_coin = ProtoCoin {
            denom: denom.to_string(),
            amount: amount.to_string(),
        };

        let msg_microtx = MsgMicrotx {
            receiver: receiver.ethermint_address.to_string(),
            sender: sender.ethermint_address.to_string(),
            amount: Some(amount_coin),
        };
        let msg = Msg::new(MSG_MICROTX_TYPE_URL, msg_microtx);

        msgs.push(msg);
        amounts.push(amount.into());
        fees.push(expected_fee.into());
        info!(
            "{}: {} (+ {}) -> {}",
            sender.ethermint_address,
            amount.to_string(),
            expected_fee.to_string(),
            receiver.ethermint_address,
        );
    }

    (msgs, amounts, fees)
}

/// Executes the given `msgs`, checking that the associated `msg_amounts` and
/// `msg_exp_fees` have been deducted from the accounts except in the situation
/// where an amount and fee total higher than the account's balance
pub async fn exec_and_check(
    contact: &Contact,
    senders: &[EthermintUserKey],
    msgs: &[Msg],
    msg_amounts: &[Uint256],
    msg_exp_fees: &[Uint256],
    token_balance: u128,
) {
    let zero_fee = Coin {
        amount: 0u8.into(),
        denom: STAKING_TOKEN.clone(),
    };
    let token_balance: Uint256 = token_balance.into();
    for (((sender, msg), amt), exp_fee) in senders
        .iter()
        .zip(msgs.iter())
        .zip(msg_amounts.iter())
        .zip(msg_exp_fees.iter())
    {
        let res = contact
            .send_message(
                &[msg.clone()],
                None,
                &[zero_fee.clone()],
                Some(OPERATION_TIMEOUT),
                sender.ethermint_key,
            )
            .await;
        if token_balance < *amt + *exp_fee {
            // FAILURE CASE
            assert!(
                res.is_err(),
                "Unexpected success when sending more than {}: address {}, amt {}, fee {}",
                token_balance,
                sender.ethermint_address,
                amt,
                exp_fee
            );
        } else {
            // SUCCESS CASE
            assert!(
                res.is_ok(),
                "Unexpected failure when sending <= {}: address {}, amt {}, fee {}: res {:?}",
                token_balance,
                sender.ethermint_address,
                amt,
                exp_fee,
                res,
            );
        }
        debug!("Sent MsgMicrotx with response {:?}", res);
    }
}

/// Asserts that the senders have appropriate reduced balances and receivers
/// have increased balances, accounting for expected failures
pub async fn assert_balance_changes(
    contact: &Contact,
    senders: &[EthermintUserKey],
    receivers: &[EthermintUserKey],
    msg_amounts: &[Uint256],
    msg_exp_fees: &[Uint256],
    token_balance: u128,
    token_denom: &str,
) {
    let token_balance: Uint256 = token_balance.into();
    for (((sender, receiver), amt), exp_fee) in senders
        .iter()
        .zip(receivers.iter())
        .zip(msg_amounts.iter())
        .zip(msg_exp_fees.iter())
    {
        let sender_bal = contact
            .get_balance(sender.ethermint_address, token_denom.to_string())
            .await
            .unwrap();
        let receiver_bal = contact
            .get_balance(receiver.ethermint_address, token_denom.to_string())
            .await
            .unwrap();
        let sender_bal = match sender_bal {
            Some(v) => v.amount,
            None => 0u8.into(),
        };
        let receiver_bal = match receiver_bal {
            Some(v) => v.amount,
            None => 0u8.into(),
        };

        if token_balance < *amt + *exp_fee {
            // FAILURE CASE
            let exp_send_bal: Uint256 = token_balance;
            let exp_recv_bal: Uint256 = 0u8.into();

            assert!(
                sender_bal == exp_send_bal && receiver_bal == exp_recv_bal,
                "Expected unchanged balances, found sender {} balance ({}), receiver {} balance ({})",
                sender.ethermint_address,
                sender_bal,
                receiver.ethermint_address,
                receiver_bal,
            );
        } else {
            // SUCCESS CASE
            let exp_send_bal: Uint256 = token_balance - *amt - *exp_fee;
            let exp_recv_bal: Uint256 = *amt;

            assert!(
                sender_bal == exp_send_bal && receiver_bal == exp_recv_bal,
                "Expected balance transfer less fee, found sender {} balance ({}), receiver {} balance ({})",
                sender.ethermint_address,
                sender_bal,
                receiver.ethermint_address,
                receiver_bal,
            );
        }
    }
}
