package ibcutils

import (
	"strings"

	sdk "github.com/cosmos/cosmos-sdk/types"
	sdkerrors "github.com/cosmos/cosmos-sdk/types/errors"

	transfertypes "github.com/cosmos/ibc-go/v4/modules/apps/transfer/types"
	channeltypes "github.com/cosmos/ibc-go/v4/modules/core/04-channel/types"
)

// GetTransferSenderRecipient returns the sender and recipient sdk.AccAddresses
// from an ICS20 FungibleTokenPacketData as well as the original sender bech32
// address from the packet data. This function fails if:
//   - the packet data is not FungibleTokenPacketData
//   - sender address is invalid
//   - recipient address is invalid
func GetTransferSenderRecipient(packet channeltypes.Packet) (
	sender, recipient sdk.AccAddress,
	senderBech32, recipientBech32 string,
	err error,
) {
	// unmarshal packet data to obtain the sender and recipient
	var data transfertypes.FungibleTokenPacketData
	if err := transfertypes.ModuleCdc.UnmarshalJSON(packet.GetData(), &data); err != nil {
		return nil, nil, "", "", sdkerrors.Wrapf(sdkerrors.ErrUnknownRequest, "cannot unmarshal ICS-20 transfer packet data")
	}

	// validate the sender bech32 address from the counterparty chain
	// and change the bech32 human readable prefix (HRP) of the sender to `althea`
	sender, err = GetAltheaAddressFromBech32(data.Sender)
	if err != nil {
		return nil, nil, "", "", sdkerrors.Wrap(err, "invalid sender")
	}

	// validate the recipient bech32 address from the counterparty chain
	// and change the bech32 human readable prefix (HRP) of the recipient to `althea`
	recipient, err = GetAltheaAddressFromBech32(data.Receiver)
	if err != nil {
		return nil, nil, "", "", sdkerrors.Wrap(err, "invalid recipient")
	}

	return sender, recipient, data.Sender, data.Receiver, nil
}

// GetTransferAmount returns the amount from an ICS20 FungibleTokenPacketData.
func GetTransferAmount(packet channeltypes.Packet) (string, error) {
	// unmarshal packet data to obtain the sender and recipient
	var data transfertypes.FungibleTokenPacketData
	if err := transfertypes.ModuleCdc.UnmarshalJSON(packet.GetData(), &data); err != nil {
		return "", sdkerrors.Wrapf(sdkerrors.ErrUnknownRequest, "cannot unmarshal ICS-20 transfer packet data")
	}

	if data.Amount == "" {
		return "", sdkerrors.Wrapf(sdkerrors.ErrInvalidCoins, "empty amount")
	}

	if _, ok := sdk.NewIntFromString(data.Amount); !ok {
		return "", sdkerrors.Wrapf(sdkerrors.ErrInvalidCoins, "invalid amount")
	}

	return data.Amount, nil
}

// GetReceivedCoin returns the transferred coin from an ICS20 FungibleTokenPacketData
// as seen from the destination chain.
// If the receiving chain is the source chain of the tokens, it removes the prefix
// path added by source (i.e sender) chain to the denom. Otherwise, it adds the
// prefix path from the destination chain to the denom.
func GetReceivedCoin(srcPort, srcChannel, dstPort, dstChannel, rawDenom, rawAmt string) sdk.Coin {
	// NOTE: Denom and amount are already validated
	amount, _ := sdk.NewIntFromString(rawAmt)

	if transfertypes.ReceiverChainIsSource(srcPort, srcChannel, rawDenom) {
		// remove prefix added by sender chain
		voucherPrefix := transfertypes.GetDenomPrefix(srcPort, srcChannel)
		unprefixedDenom := rawDenom[len(voucherPrefix):]

		// coin denomination used in sending from the escrow address
		denom := unprefixedDenom

		// The denomination used to send the coins is either the native denom or the hash of the path
		// if the denomination is not native.
		denomTrace := transfertypes.ParseDenomTrace(unprefixedDenom)
		if denomTrace.Path != "" {
			denom = denomTrace.IBCDenom()
		}

		return sdk.Coin{
			Denom:  denom,
			Amount: amount,
		}
	}

	// since SendPacket did not prefix the denomination, we must prefix denomination here
	sourcePrefix := transfertypes.GetDenomPrefix(dstPort, dstChannel)
	// NOTE: sourcePrefix contains the trailing "/"
	prefixedDenom := sourcePrefix + rawDenom

	// construct the denomination trace from the full raw denomination
	denomTrace := transfertypes.ParseDenomTrace(prefixedDenom)
	voucherDenom := denomTrace.IBCDenom()

	return sdk.Coin{
		Denom:  voucherDenom,
		Amount: amount,
	}
}

// GetAltheaAddressFromBech32 returns the sdk.Account address of given address,
// while also changing bech32 human readable prefix (HRP) to the value set on
// the global sdk.Config (eg: `althea`).
// The function fails if the provided bech32 address is invalid.
func GetAltheaAddressFromBech32(address string) (sdk.AccAddress, error) {
	bech32Prefix := strings.SplitN(address, "1", 2)[0]
	if bech32Prefix == address {
		return nil, sdkerrors.Wrapf(sdkerrors.ErrInvalidAddress, "invalid bech32 address: %s", address)
	}

	addressBz, err := sdk.GetFromBech32(address, bech32Prefix)
	if err != nil {
		return nil, sdkerrors.Wrapf(sdkerrors.ErrInvalidAddress, "invalid address %s, %s", address, err.Error())
	}

	// safety check: shouldn't happen
	if err := sdk.VerifyAddressFormat(addressBz); err != nil {
		return nil, err
	}

	return sdk.AccAddress(addressBz), nil
}
