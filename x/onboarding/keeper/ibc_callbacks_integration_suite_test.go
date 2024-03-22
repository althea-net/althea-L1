package keeper_test

import (
	"strconv"
	"testing"

	evmtypes "github.com/evmos/ethermint/x/evm/types"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	"github.com/stretchr/testify/suite"

	sdk "github.com/cosmos/cosmos-sdk/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	minttypes "github.com/cosmos/cosmos-sdk/x/mint/types"

	transfertypes "github.com/cosmos/ibc-go/v4/modules/apps/transfer/types"
	clienttypes "github.com/cosmos/ibc-go/v4/modules/core/02-client/types"
	channeltypes "github.com/cosmos/ibc-go/v4/modules/core/04-channel/types"

	erc20types "github.com/Canto-Network/Canto/v5/x/erc20/types"

	althea "github.com/althea-net/althea-L1/app"
	ibcgotesting "github.com/althea-net/althea-L1/ibcutils/testing"
	onboardingtest "github.com/althea-net/althea-L1/x/onboarding/testutil"
	onboardingtypes "github.com/althea-net/althea-L1/x/onboarding/types"
)

type IBCTestingSuite struct {
	suite.Suite
	coordinator *ibcgotesting.Coordinator

	// testing chains used for convenience and readability
	AltheaChain     *ibcgotesting.TestChain
	IBCGravityChain *ibcgotesting.TestChain
	IBCCosmosChain  *ibcgotesting.TestChain

	PathGravityAlthea *ibcgotesting.Path
	PathCosmosAlthea  *ibcgotesting.Path
	PathGravityCosmos *ibcgotesting.Path
}

var s *IBCTestingSuite

func TestIBCTestingSuite(t *testing.T) {
	s = new(IBCTestingSuite)
	suite.Run(t, s)

	// Run Ginkgo integration tests
	RegisterFailHandler(Fail)
	RunSpecs(t, "Keeper Suite")
}

func (suite *IBCTestingSuite) SetupTest() {
	// initializes 3 test chains
	suite.coordinator = ibcgotesting.NewCoordinator(suite.T(), 1, 2)
	suite.AltheaChain = suite.coordinator.GetChain(ibcgotesting.GetChainIDAlthea(1))
	suite.IBCGravityChain = suite.coordinator.GetChain(ibcgotesting.GetChainID(2))
	suite.IBCCosmosChain = suite.coordinator.GetChain(ibcgotesting.GetChainID(3))
	suite.coordinator.CommitNBlocks(suite.AltheaChain, 2)
	suite.coordinator.CommitNBlocks(suite.IBCGravityChain, 2)
	suite.coordinator.CommitNBlocks(suite.IBCCosmosChain, 2)

	// Mint coins on the gravity side which we'll use to unlock our aalthea
	coinUsdc := sdk.NewCoin("uUSDC", sdk.NewIntWithDecimal(10000, 6))
	coinUsdt := sdk.NewCoin("uUSDT", sdk.NewIntWithDecimal(10000, 6))
	coins := sdk.NewCoins(coinUsdc, coinUsdt)
	err := suite.IBCGravityChain.GetSimApp().BankKeeper.MintCoins(suite.IBCGravityChain.GetContext(), minttypes.ModuleName, coins)
	suite.Require().NoError(err)
	err = suite.IBCGravityChain.GetSimApp().BankKeeper.SendCoinsFromModuleToAccount(suite.IBCGravityChain.GetContext(), minttypes.ModuleName, suite.IBCGravityChain.SenderAccount.GetAddress(), coins)
	suite.Require().NoError(err)

	// Mint coins on the cosmos side which we'll use to unlock our aalthea
	coinAtom := sdk.NewCoin("uatom", sdk.NewIntWithDecimal(10000, 6))
	coins = sdk.NewCoins(coinAtom)
	err = suite.IBCCosmosChain.GetSimApp().BankKeeper.MintCoins(suite.IBCCosmosChain.GetContext(), minttypes.ModuleName, coins)
	suite.Require().NoError(err)
	err = suite.IBCCosmosChain.GetSimApp().BankKeeper.SendCoinsFromModuleToAccount(suite.IBCCosmosChain.GetContext(), minttypes.ModuleName, suite.IBCCosmosChain.SenderAccount.GetAddress(), coins)
	suite.Require().NoError(err)

	params := onboardingtypes.DefaultParams()
	params.EnableOnboarding = true
	suite.AltheaChain.App.(*althea.AltheaApp).OnboardingKeeper.SetParams(suite.AltheaChain.GetContext(), params)

	// Setup the paths between the chains
	suite.PathGravityAlthea = ibcgotesting.NewTransferPath(suite.IBCGravityChain, suite.AltheaChain) // clientID, connectionID, channelID empty
	suite.PathCosmosAlthea = ibcgotesting.NewTransferPath(suite.IBCCosmosChain, suite.AltheaChain)
	suite.PathGravityCosmos = ibcgotesting.NewTransferPath(suite.IBCCosmosChain, suite.IBCGravityChain)
	suite.coordinator.Setup(suite.PathGravityAlthea) // clientID, connectionID, channelID filled
	suite.coordinator.Setup(suite.PathCosmosAlthea)
	suite.coordinator.Setup(suite.PathGravityCosmos)
	suite.Require().Equal("07-tendermint-0", suite.PathGravityAlthea.EndpointA.ClientID)
	suite.Require().Equal("connection-0", suite.PathGravityAlthea.EndpointA.ConnectionID)
	suite.Require().Equal("channel-0", suite.PathGravityAlthea.EndpointA.ChannelID)

	// Set the proposer address for the current header
	// It because EVMKeeper.GetCoinbaseAddress requires ProposerAddress in block header
	suite.AltheaChain.CurrentHeader.ProposerAddress = suite.AltheaChain.LastHeader.ValidatorSet.Proposer.Address
	suite.IBCGravityChain.CurrentHeader.ProposerAddress = suite.IBCGravityChain.LastHeader.ValidatorSet.Proposer.Address
	suite.IBCCosmosChain.CurrentHeader.ProposerAddress = suite.IBCCosmosChain.LastHeader.ValidatorSet.Proposer.Address
}

// FundAltheaChain mints coins and sends them to the AltheaChain sender account
func (suite *IBCTestingSuite) FundAltheaChain(coins sdk.Coins) {
	err := suite.AltheaChain.App.(*althea.AltheaApp).BankKeeper.MintCoins(suite.AltheaChain.GetContext(), evmtypes.ModuleName, coins)
	suite.Require().NoError(err)
	err = suite.AltheaChain.App.(*althea.AltheaApp).BankKeeper.SendCoinsFromModuleToAccount(suite.AltheaChain.GetContext(), evmtypes.ModuleName, suite.AltheaChain.SenderAccount.GetAddress(), coins)
	suite.Require().NoError(err)
}

// setupRegisterCoin deploys an erc20 contract and creates the token pair
func (suite *IBCTestingSuite) setupRegisterCoin(metadata banktypes.Metadata) *erc20types.TokenPair {
	err := suite.AltheaChain.App.(*althea.AltheaApp).BankKeeper.MintCoins(suite.AltheaChain.GetContext(), evmtypes.ModuleName, sdk.Coins{sdk.NewInt64Coin(metadata.Base, 1)})
	suite.Require().NoError(err)

	pair, err := suite.AltheaChain.App.(*althea.AltheaApp).Erc20Keeper.RegisterCoin(suite.AltheaChain.GetContext(), metadata)
	suite.Require().NoError(err)
	return pair
}

var (
	timeoutHeight   = clienttypes.NewHeight(1000, 1000)
	uusdcDenomtrace = transfertypes.DenomTrace{
		Path:      "transfer/channel-0",
		BaseDenom: "uUSDC",
	}
	uusdcIbcdenom = uusdcDenomtrace.IBCDenom()

	uusdcCh100DenomTrace = transfertypes.DenomTrace{
		Path:      "transfer/channel-100",
		BaseDenom: "uUSDC",
	}
	uusdcCh100IbcDenom = uusdcCh100DenomTrace.IBCDenom()

	uusdtDenomtrace = transfertypes.DenomTrace{
		Path:      "transfer/channel-0",
		BaseDenom: "uUSDT",
	}
	uusdtIbcdenom = uusdtDenomtrace.IBCDenom()

	uatomDenomtrace = transfertypes.DenomTrace{
		Path:      "transfer/channel-1",
		BaseDenom: "uatom",
	}
	uatomIbcdenom = uatomDenomtrace.IBCDenom()
)

// SendAndReceiveMessage sends a transfer message from the origin chain to the destination chain
func (suite *IBCTestingSuite) SendAndReceiveMessage(path *ibcgotesting.Path, origin *ibcgotesting.TestChain, coin string, amount int64, sender string, receiver string, seq uint64) *sdk.Result {
	// Send coin from A to B
	transferMsg := transfertypes.NewMsgTransfer(path.EndpointA.ChannelConfig.PortID, path.EndpointA.ChannelID, sdk.NewCoin(coin, sdk.NewInt(amount)), sender, receiver, timeoutHeight, 0)
	_, err := origin.SendMsgs(transferMsg)
	suite.Require().NoError(err) // message committed

	// Recreate the packet that was sent
	transfer := transfertypes.NewFungibleTokenPacketData(coin, strconv.Itoa(int(amount)), sender, receiver)
	packet := channeltypes.NewPacket(transfer.GetBytes(), seq, path.EndpointA.ChannelConfig.PortID, path.EndpointA.ChannelID, path.EndpointB.ChannelConfig.PortID, path.EndpointB.ChannelID, timeoutHeight, 0)

	// patched RelayPacket call to get res
	res, err := onboardingtest.RelayPacket(path, packet)

	suite.Require().NoError(err)
	return res
}
