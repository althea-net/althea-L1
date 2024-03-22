package keeper_test

import (
	"fmt"
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/evmos/ethermint/crypto/ethsecp256k1"
	evmtypes "github.com/evmos/ethermint/x/evm/types"
	"github.com/stretchr/testify/mock"

	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	sdk "github.com/cosmos/cosmos-sdk/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	distrtypes "github.com/cosmos/cosmos-sdk/x/distribution/types"

	transfertypes "github.com/cosmos/ibc-go/v4/modules/apps/transfer/types"
	clienttypes "github.com/cosmos/ibc-go/v4/modules/core/02-client/types"
	channeltypes "github.com/cosmos/ibc-go/v4/modules/core/04-channel/types"
	ibcgotesting "github.com/cosmos/ibc-go/v4/testing"
	ibcmock "github.com/cosmos/ibc-go/v4/testing/mock"

	"github.com/Canto-Network/Canto/v5/contracts"
	erc20types "github.com/Canto-Network/Canto/v5/x/erc20/types"

	"github.com/althea-net/althea-L1/x/onboarding/keeper"
	onboardingtest "github.com/althea-net/althea-L1/x/onboarding/testutil"
	onboardingtypes "github.com/althea-net/althea-L1/x/onboarding/types"
)

var (
	metadataIbcUSDC = banktypes.Metadata{
		Description: "USDC IBC voucher (channel 0)",
		Base:        uusdcIbcdenom,
		// NOTE: Denom units MUST be increasing
		DenomUnits: []*banktypes.DenomUnit{
			{
				Denom:    uusdcIbcdenom,
				Exponent: 0,
			},
		},
		Name:    "USDC channel-0",
		Symbol:  "ibcUSDC-0",
		Display: uusdcIbcdenom,
	}

	metadataIbcUSDT = banktypes.Metadata{
		Description: "USDT IBC voucher (channel 0)",
		Base:        uusdtIbcdenom,
		// NOTE: Denom units MUST be increasing
		DenomUnits: []*banktypes.DenomUnit{
			{
				Denom:    uusdtIbcdenom,
				Exponent: 0,
			},
		},
		Name:    "USDT channel-0",
		Symbol:  "ibcUSDT-0",
		Display: uusdtIbcdenom,
	}
)

// setupRegisterCoin is a helper function for registering a new ERC20 token pair using the Erc20Keeper.
func (suite *KeeperTestSuite) setupRegisterCoin(metadata banktypes.Metadata) *erc20types.TokenPair {
	pair, err := suite.app.Erc20Keeper.RegisterCoin(suite.ctx, metadata)
	suite.Require().NoError(err)
	return pair
}

func (suite *KeeperTestSuite) TestOnRecvPacket() {
	// secp256k1 account
	secpPk := secp256k1.GenPrivKey()
	secpAddr := sdk.AccAddress(secpPk.PubKey().Address())
	secpAddrCosmos := sdk.MustBech32ifyAddressBytes(sdk.Bech32MainPrefix, secpAddr)

	// ethsecp256k1 account
	ethPk, err := ethsecp256k1.GenerateKey()
	suite.Require().Nil(err)
	ethsecpAddr := sdk.AccAddress(ethPk.PubKey().Address())
	ethsecpAddrAlthea := sdk.AccAddress(ethPk.PubKey().Address()).String()

	// Setup Cosmos <=> althea IBC relayer
	denom := "uUSDC"
	ibcDenom := uusdcIbcdenom
	transferAmount := sdk.NewIntWithDecimal(25, 6)
	sourceChannel := "channel-0"
	altheaChannel := "channel-0"
	path := fmt.Sprintf("%s/%s", transfertypes.PortID, altheaChannel)

	timeoutHeight := clienttypes.NewHeight(0, 100)
	disabledTimeoutTimestamp := uint64(0)
	mockPacket := channeltypes.NewPacket(ibcgotesting.MockPacketData, 1, transfertypes.PortID, sourceChannel, transfertypes.PortID, altheaChannel, timeoutHeight, disabledTimeoutTimestamp)
	packet := mockPacket
	expAck := ibcmock.MockAcknowledgement

	testCases := []struct {
		name              string
		malleate          func()
		ackSuccess        bool
		expVoucherBalance sdk.Coin
		expErc20Balance   sdk.Int
	}{
		{
			"fail - invalid sender - missing '1' ",
			func() {
				transfer := transfertypes.NewFungibleTokenPacketData(denom, "100", "althea", ethsecpAddrAlthea)
				bz := transfertypes.ModuleCdc.MustMarshalJSON(&transfer)
				packet = channeltypes.NewPacket(bz, 100, transfertypes.PortID, sourceChannel, transfertypes.PortID, altheaChannel, timeoutHeight, 0)
			},
			false,
			sdk.NewCoin(uusdcIbcdenom, transferAmount),
			sdk.ZeroInt(),
		},
		{
			"fail - invalid sender - invalid bech32",
			func() {
				transfer := transfertypes.NewFungibleTokenPacketData(denom, "100", "badba1sv9m0g7ycejwr3s369km58h5qe7xj77hvcxrms", ethsecpAddrAlthea)
				bz := transfertypes.ModuleCdc.MustMarshalJSON(&transfer)
				packet = channeltypes.NewPacket(bz, 100, transfertypes.PortID, sourceChannel, transfertypes.PortID, altheaChannel, timeoutHeight, 0)
			},
			false,
			sdk.NewCoin(uusdcIbcdenom, transferAmount),
			sdk.ZeroInt(),
		},
		{
			"continue - receiver is a module account",
			func() {
				distrAcc := suite.app.AccountKeeper.GetModuleAccount(suite.ctx, distrtypes.ModuleName)
				suite.Require().NotNil(distrAcc)
				addr := distrAcc.GetAddress().String()
				transfer := transfertypes.NewFungibleTokenPacketData(denom, "100", addr, addr)
				bz := transfertypes.ModuleCdc.MustMarshalJSON(&transfer)
				packet = channeltypes.NewPacket(bz, 100, transfertypes.PortID, sourceChannel, transfertypes.PortID, altheaChannel, timeoutHeight, 0)
			},
			true,
			sdk.NewCoin(uusdcIbcdenom, transferAmount),
			sdk.ZeroInt(),
		},
		{
			"continue - params disabled",
			func() {
				params := suite.app.OnboardingKeeper.GetParams(suite.ctx)
				params.EnableOnboarding = false
				suite.app.OnboardingKeeper.SetParams(suite.ctx, params)
			},
			true,
			sdk.NewCoin(uusdcIbcdenom, transferAmount),
			sdk.ZeroInt(),
		},
		{
			"convert all transferred amount",
			func() {

				denom = "uUSDT"
				ibcDenom = uusdtIbcdenom

				transfer := transfertypes.NewFungibleTokenPacketData(denom, transferAmount.String(), secpAddrCosmos, ethsecpAddrAlthea)
				bz := transfertypes.ModuleCdc.MustMarshalJSON(&transfer)
				packet = channeltypes.NewPacket(bz, 100, transfertypes.PortID, sourceChannel, transfertypes.PortID, altheaChannel, timeoutHeight, 0)
			},
			true,
			sdk.NewCoin(uusdtIbcdenom, sdk.ZeroInt()),
			transferAmount,
		},
		{
			"no convert - unauthorized  channel",
			func() {
				denom = uusdcCh100DenomTrace.BaseDenom
				ibcDenom = uusdcCh100IbcDenom
				altheaChannel = "channel-100"
				transferAmount = sdk.NewIntWithDecimal(25, 6)
				transfer := transfertypes.NewFungibleTokenPacketData(denom, transferAmount.String(), secpAddrCosmos, ethsecpAddrAlthea)
				bz := transfertypes.ModuleCdc.MustMarshalJSON(&transfer)
				packet = channeltypes.NewPacket(bz, 100, transfertypes.PortID, sourceChannel, transfertypes.PortID, altheaChannel, timeoutHeight, 0)
			},
			true,
			sdk.NewCoin(uusdcCh100IbcDenom, transferAmount),
			sdk.ZeroInt(),
		},
		{
			"convert fail",
			func() {
				denom = uusdcDenomtrace.BaseDenom
				ibcDenom = uusdcIbcdenom

				altheaChannel = "channel-0"
				transferAmount = sdk.NewIntWithDecimal(25, 6)
				transfer := transfertypes.NewFungibleTokenPacketData(denom, transferAmount.String(), secpAddrCosmos, ethsecpAddrAlthea)
				bz := transfertypes.ModuleCdc.MustMarshalJSON(&transfer)
				packet = channeltypes.NewPacket(bz, 100, transfertypes.PortID, sourceChannel, transfertypes.PortID, altheaChannel, timeoutHeight, 0)

				pairID := suite.app.Erc20Keeper.GetTokenPairID(suite.ctx, metadataIbcUSDC.Base)
				pair, _ := suite.app.Erc20Keeper.GetTokenPair(suite.ctx, pairID)
				pair.Enabled = false
				suite.app.Erc20Keeper.SetTokenPair(suite.ctx, pair)

			},
			true,
			sdk.NewCoin(uusdcIbcdenom, sdk.NewIntWithDecimal(25, 6)),
			sdk.NewInt(0),
		},
	}
	for _, tc := range testCases {
		suite.Run(fmt.Sprintf("Case %s", tc.name), func() {
			suite.SetupTest() // reset

			coins := sdk.NewCoins(sdk.NewCoin("aalthea", sdk.NewIntWithDecimal(10000, 18)), sdk.NewCoin(uusdcIbcdenom, sdk.NewIntWithDecimal(10000, 6)))
			suite.Require().NoError(suite.app.BankKeeper.MintCoins(suite.ctx, evmtypes.ModuleName, coins))
			suite.app.BankKeeper.SendCoinsFromModuleToAccount(suite.ctx, evmtypes.ModuleName, secpAddr, coins)

			// Enable Onboarding
			params := suite.app.OnboardingKeeper.GetParams(suite.ctx)
			params.EnableOnboarding = true
			params.WhitelistedChannels = []string{"channel-0"}
			suite.app.OnboardingKeeper.SetParams(suite.ctx, params)

			// Deploy ERC20 Contract
			err := suite.app.BankKeeper.MintCoins(suite.ctx, evmtypes.ModuleName, sdk.Coins{sdk.NewInt64Coin(metadataIbcUSDC.Base, 1)})
			suite.Require().NoError(err)
			usdcPair := suite.setupRegisterCoin(metadataIbcUSDC)
			suite.Require().NotNil(usdcPair)
			suite.app.Erc20Keeper.SetTokenPair(suite.ctx, *usdcPair)

			err = suite.app.BankKeeper.MintCoins(suite.ctx, evmtypes.ModuleName, sdk.Coins{sdk.NewInt64Coin(metadataIbcUSDT.Base, 1)})
			suite.Require().NoError(err)
			usdtPair := suite.setupRegisterCoin(metadataIbcUSDT)
			suite.Require().NotNil(usdtPair)
			suite.app.Erc20Keeper.SetTokenPair(suite.ctx, *usdtPair)

			tc.malleate()

			// Set Denom Trace
			denomTrace := transfertypes.DenomTrace{
				Path:      path,
				BaseDenom: denom,
			}
			suite.app.IbcTransferKeeper.SetDenomTrace(suite.ctx, denomTrace)

			// Set Cosmos Channel
			channel := channeltypes.Channel{
				State:          channeltypes.INIT,
				Ordering:       channeltypes.UNORDERED,
				Counterparty:   channeltypes.NewCounterparty(transfertypes.PortID, sourceChannel),
				ConnectionHops: []string{sourceChannel},
			}
			suite.app.IbcKeeper.ChannelKeeper.SetChannel(suite.ctx, transfertypes.PortID, altheaChannel, channel)

			// Set Next Sequence Send
			suite.app.IbcKeeper.ChannelKeeper.SetNextSequenceSend(suite.ctx, transfertypes.PortID, altheaChannel, 1)

			// Mock the Transferkeeper to always return nil on SendTransfer(), as this
			// method requires a successfull handshake with the counterparty chain.
			// This, however, exceeds the requirements of the unit tests.
			mockTransferKeeper := &MockTransferKeeper{
				Keeper: suite.app.BankKeeper,
			}

			mockTransferKeeper.On("GetDenomTrace", mock.Anything, mock.Anything).Return(denomTrace, true)
			mockTransferKeeper.On("SendTransfer", mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything, mock.Anything).Return(nil)

			sp, found := suite.app.ParamsKeeper.GetSubspace(onboardingtypes.ModuleName)
			suite.Require().True(found)
			suite.app.OnboardingKeeper = keeper.NewKeeper(sp, suite.app.AccountKeeper, suite.app.BankKeeper, suite.app.Erc20Keeper, suite.app.IbcKeeper.ChannelKeeper)

			// Fund receiver account with the transferred amount
			coins = sdk.NewCoins(sdk.NewCoin(ibcDenom, transferAmount))
			suite.Require().NoError(suite.app.BankKeeper.MintCoins(suite.ctx, evmtypes.ModuleName, coins))
			suite.app.BankKeeper.SendCoinsFromModuleToAccount(suite.ctx, evmtypes.ModuleName, ethsecpAddr, coins)

			// Perform IBC callback
			ack := suite.app.OnboardingKeeper.OnRecvPacket(suite.ctx, packet, expAck)

			// Check acknowledgement
			if tc.ackSuccess {
				suite.Require().True(ack.Success(), string(ack.Acknowledgement()))
				suite.Require().Equal(expAck, ack)
			} else {
				suite.Require().False(ack.Success(), string(ack.Acknowledgement()))
			}

			// Check balances
			voucherBalance := suite.app.BankKeeper.GetBalance(suite.ctx, ethsecpAddr, ibcDenom)
			var erc20balance *big.Int

			if ibcDenom == uusdcIbcdenom {
				erc20balance = suite.app.Erc20Keeper.BalanceOf(suite.ctx, contracts.ERC20MinterBurnerDecimalsContract.ABI, usdcPair.GetERC20Contract(), common.BytesToAddress(ethsecpAddr.Bytes()))
			} else {
				erc20balance = suite.app.Erc20Keeper.BalanceOf(suite.ctx, contracts.ERC20MinterBurnerDecimalsContract.ABI, usdtPair.GetERC20Contract(), common.BytesToAddress(ethsecpAddr.Bytes()))
			}

			suite.Require().Equal(tc.expVoucherBalance, voucherBalance)
			suite.Require().Equal(tc.expErc20Balance.String(), erc20balance.String())

			events := suite.ctx.EventManager().Events()

			attrs := onboardingtest.ExtractAttributes(onboardingtest.FindEvent(events, "convert_coin"))

			if tc.expErc20Balance.IsPositive() {
				// Check that the amount of ERC20 tokens minted is equal to the difference between
				// the transferred amount and the swapped amount
				suite.Require().Equal(tc.expErc20Balance.String(), transferAmount.String())
				suite.Require().Equal(tc.expErc20Balance.String(), attrs["amount"])
			} else {
				suite.Require().Equal(0, len(attrs))
			}
		})
	}
}
