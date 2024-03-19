package keeper

import (
	"github.com/ethereum/go-ethereum/common"

	errorsmod "cosmossdk.io/errors"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	transfertypes "github.com/cosmos/ibc-go/v4/modules/apps/transfer/types"
	channeltypes "github.com/cosmos/ibc-go/v4/modules/core/04-channel/types"
	"github.com/cosmos/ibc-go/v4/modules/core/exported"

	erc20types "github.com/Canto-Network/Canto/v5/x/erc20/types"

	"github.com/althea-net/althea-L1/ibcutils"
	"github.com/althea-net/althea-L1/x/onboarding/types"
)

// OnRecvPacket performs an IBC receive callback.
// It swaps the transferred IBC denom ERC20 tokens where applicable.
func (k Keeper) OnRecvPacket(
	ctx sdk.Context,
	packet channeltypes.Packet,
	ack exported.Acknowledgement,
) exported.Acknowledgement {
	logger := k.Logger(ctx)

	// It always returns original ACK
	// meaning that even if the conversion fails, it does not revert IBC transfer
	// and the asset transferred to Althea-L1 will still remain on Althea-L1

	params := k.GetParams(ctx)
	if !params.EnableOnboarding {
		return ack
	}

	// check source channel is in the whitelist channels
	var found bool
	for _, s := range params.WhitelistedChannels {
		if s == packet.DestinationChannel {
			found = true
		}
	}

	if !found {
		return ack
	}

	// Get recipient addresses in `althea1...` and the original bech32 format
	_, recipient, senderBech32, recipientBech32, err := ibcutils.GetTransferSenderRecipient(packet)
	if err != nil {
		return channeltypes.NewErrorAcknowledgement(err)
	}

	// get the recipient account
	account := k.AccountKeeper.GetAccount(ctx, recipient)

	// onboarding is not supported for module accounts
	if _, isModuleAccount := account.(authtypes.ModuleAccountI); isModuleAccount {
		return ack
	}

	var data transfertypes.FungibleTokenPacketData
	if err = transfertypes.ModuleCdc.UnmarshalJSON(packet.GetData(), &data); err != nil {
		// NOTE: shouldn't happen as the packet has already
		// been decoded on ICS20 transfer logic
		err = errorsmod.Wrapf(types.ErrInvalidType, "cannot unmarshal ICS-20 transfer packet data")
		return channeltypes.NewErrorAcknowledgement(err)
	}

	// parse the transferred denom
	transferredCoin := ibcutils.GetReceivedCoin(
		packet.SourcePort, packet.SourceChannel,
		packet.DestinationPort, packet.DestinationChannel,
		data.Denom, data.Amount,
	)

	//convert coins to ERC20 token
	pairID := k.Erc20Keeper.GetTokenPairID(ctx, transferredCoin.Denom)
	if len(pairID) == 0 {
		// short-circuit: if the denom is not registered, conversion will fail
		// so we can continue with the rest of the stack
		return ack
	}

	pair, exists := k.Erc20Keeper.GetTokenPair(ctx, pairID)
	if !exists || !pair.Enabled {
		// no-op: continue with the rest of the stack without conversion
		return ack
	}

	// Build MsgConvertCoin, from recipient to recipient since IBC transfer already occurred
	convertMsg := erc20types.NewMsgConvertCoin(transferredCoin, common.BytesToAddress(recipient.Bytes()), recipient)

	// NOTE: don't call ValidateBasic since we've already validated the ICS20 packet data

	// Use MsgConvertCoin to convert the Cosmos Coin to an ERC20
	if _, err = k.Erc20Keeper.ConvertCoin(sdk.WrapSDKContext(ctx), convertMsg); err != nil {
		logger.Error("failed to convert coins", "error", err)
		return ack
	}

	logger.Info(
		"erc20 conversion completed",
		"sender", senderBech32,
		"receiver", recipientBech32,
		"source-port", packet.SourcePort,
		"source-channel", packet.SourceChannel,
		"dest-port", packet.DestinationPort,
		"dest-channel", packet.DestinationChannel,
		"convert amount", transferredCoin.Amount,
	)

	ctx.EventManager().EmitEvent(
		sdk.NewEvent(
			types.EventTypeOnboarding,
			sdk.NewAttribute(sdk.AttributeKeySender, senderBech32),
			sdk.NewAttribute(transfertypes.AttributeKeyReceiver, recipientBech32),
			sdk.NewAttribute(channeltypes.AttributeKeySrcChannel, packet.SourceChannel),
			sdk.NewAttribute(channeltypes.AttributeKeySrcPort, packet.SourcePort),
			sdk.NewAttribute(channeltypes.AttributeKeyDstPort, packet.DestinationPort),
			sdk.NewAttribute(channeltypes.AttributeKeyDstChannel, packet.DestinationChannel),
			sdk.NewAttribute(types.AttributeKeyConvertAmount, transferredCoin.Amount.String()),
		),
	)

	// return original acknowledgement
	return ack
}
