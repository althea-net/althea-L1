package keeper

import (
	"fmt"

	"github.com/tendermint/tendermint/libs/log"

	sdk "github.com/cosmos/cosmos-sdk/types"
	capabilitytypes "github.com/cosmos/cosmos-sdk/x/capability/types"
	paramtypes "github.com/cosmos/cosmos-sdk/x/params/types"

	transfertypes "github.com/cosmos/ibc-go/v4/modules/apps/transfer/types"
	porttypes "github.com/cosmos/ibc-go/v4/modules/core/05-port/types"
	"github.com/cosmos/ibc-go/v4/modules/core/exported"

	"github.com/althea-net/althea-L1/x/onboarding/types"
)

var _ transfertypes.ICS4Wrapper = Keeper{}

// Keeper struct
type Keeper struct {
	ParamStore    paramtypes.Subspace
	AccountKeeper types.AccountKeeper
	BankKeeper    types.BankKeeper
	Ics4Wrapper   porttypes.ICS4Wrapper
	Erc20Keeper   types.Erc20Keeper
}

// NewKeeper returns keeper
func NewKeeper(
	ps paramtypes.Subspace,
	ak types.AccountKeeper,
	bk types.BankKeeper,
	ek types.Erc20Keeper,
	i4 porttypes.ICS4Wrapper,
) *Keeper {
	// set KeyTable if it has not already been set
	if !ps.HasKeyTable() {
		ps = ps.WithKeyTable(types.ParamKeyTable())
	}

	return &Keeper{
		ParamStore:    ps,
		AccountKeeper: ak,
		BankKeeper:    bk,
		Erc20Keeper:   ek,
		Ics4Wrapper:   i4,
	}
}

func (k Keeper) Validate() {
	if k.AccountKeeper == nil {
		panic("Nil account keeper")
	}
	if k.BankKeeper == nil {
		panic("Nil bank keeper")
	}
	if k.Ics4Wrapper == nil {
		panic("Nil ICS4 wrapper")
	}
	if k.Erc20Keeper == nil {
		panic("Nil erc20 keeper")
	}
}

// Logger returns logger
func (k Keeper) Logger(ctx sdk.Context) log.Logger {
	return ctx.Logger().With("module", fmt.Sprintf("x/%s", types.ModuleName))
}

// IBC callbacks and transfer handlers

// SendPacket implements the ICS4Wrapper interface from the transfer module.
// It calls the underlying SendPacket function directly to move down the middleware stack.
func (k Keeper) SendPacket(ctx sdk.Context, channelCap *capabilitytypes.Capability, packet exported.PacketI) error {
	return k.Ics4Wrapper.SendPacket(ctx, channelCap, packet)
}

// WriteAcknowledgement implements the ICS4Wrapper interface from the transfer module.
// It calls the underlying WriteAcknowledgement function directly to move down the middleware stack.
func (k Keeper) WriteAcknowledgement(ctx sdk.Context, channelCap *capabilitytypes.Capability, packet exported.PacketI, ack exported.Acknowledgement) error {
	return k.Ics4Wrapper.WriteAcknowledgement(ctx, channelCap, packet, ack)
}

func (k Keeper) GetAppVersion(ctx sdk.Context, portID, channelID string) (string, bool) {
	return k.Ics4Wrapper.GetAppVersion(ctx, portID, channelID)
}
