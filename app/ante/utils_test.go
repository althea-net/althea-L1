package ante_test

import (
	"math/big"
	"testing"
	"time"

	tmproto "github.com/tendermint/tendermint/proto/tendermint/types"

	"github.com/ethereum/go-ethereum/common"
	ethtypes "github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/stretchr/testify/suite"

	client "github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/tx"
	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	cryptotypes "github.com/cosmos/cosmos-sdk/crypto/types"
	"github.com/cosmos/cosmos-sdk/simapp"
	"github.com/cosmos/cosmos-sdk/testutil/testdata"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/tx/signing"
	"github.com/cosmos/cosmos-sdk/x/auth/legacy/legacytx"
	authsigning "github.com/cosmos/cosmos-sdk/x/auth/signing"
	authtx "github.com/cosmos/cosmos-sdk/x/auth/tx"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	stakingtypes "github.com/cosmos/cosmos-sdk/x/staking/types"

	"github.com/evmos/ethermint/app"
	"github.com/evmos/ethermint/crypto/ethsecp256k1"
	"github.com/evmos/ethermint/encoding"
	"github.com/evmos/ethermint/ethereum/eip712"
	"github.com/evmos/ethermint/tests"
	"github.com/evmos/ethermint/types"
	"github.com/evmos/ethermint/x/evm/statedb"
	evmtypes "github.com/evmos/ethermint/x/evm/types"
	feemarkettypes "github.com/evmos/ethermint/x/feemarket/types"

	cantoante "github.com/Canto-Network/Canto/v5/app/ante"

	althea "github.com/althea-net/althea-L1/app"
	ante "github.com/althea-net/althea-L1/app/ante"
	altheaconfig "github.com/althea-net/althea-L1/config"
	microtxtypes "github.com/althea-net/althea-L1/x/microtx/types"
)

type AnteTestSuite struct {
	suite.Suite

	ctx             sdk.Context
	app             *althea.AltheaApp
	clientCtx       client.Context
	anteHandler     sdk.AnteHandler
	oldAnteHandler  sdk.AnteHandler
	ethSigner       ethtypes.Signer
	enableFeemarket bool
	evmParamsOption func(*evmtypes.Params)
}

const TestGasLimit uint64 = 1000000

func (suite *AnteTestSuite) StateDB() *statedb.StateDB {
	return statedb.New(suite.ctx, suite.app.EvmKeeper, statedb.NewEmptyTxConfig(common.BytesToHash(suite.ctx.HeaderHash().Bytes())))
}

// Runs the app initialization and sets up several module params to get the test chain in a runnable state
func (suite *AnteTestSuite) SetupTest() {
	checkTx := false
	cfg := sdk.GetConfig()
	cfg.SetBech32PrefixForAccount("althea", "altheapub")

	suite.app = althea.Setup(checkTx, func(app *althea.AltheaApp, genesis althea.GenesisState) althea.GenesisState {
		if suite.enableFeemarket {
			// setup feemarketGenesis params
			feemarketGenesis := feemarkettypes.DefaultGenesisState()
			feemarketGenesis.Params.EnableHeight = 1
			feemarketGenesis.Params.NoBaseFee = false
			// Verify feeMarket genesis
			err := feemarketGenesis.Validate()
			suite.Require().NoError(err)
			genesis[feemarkettypes.ModuleName] = app.AppCodec().MustMarshalJSON(feemarketGenesis)
		}
		evmGenesis := evmtypes.DefaultGenesisState()
		evmGenesis.Params.EvmDenom = altheaconfig.BaseDenom
		evmGenesis.Params.AllowUnprotectedTxs = false

		if suite.evmParamsOption != nil {
			suite.evmParamsOption(&evmGenesis.Params)
		}
		genesis[evmtypes.ModuleName] = app.AppCodec().MustMarshalJSON(evmGenesis)
		return genesis
	})

	// nolint: exhaustruct
	suite.ctx = suite.app.BaseApp.NewContext(checkTx, tmproto.Header{Height: 2, ChainID: "althea_417834-1", Time: time.Now().UTC()})
	suite.ctx = suite.ctx.WithMinGasPrices(sdk.NewDecCoins(sdk.NewDecCoin(altheaconfig.BaseDenom, sdk.OneInt())))
	suite.ctx = suite.ctx.WithBlockGasMeter(sdk.NewGasMeter(1000000000000000000))

	infCtx := suite.ctx.WithGasMeter(sdk.NewInfiniteGasMeter())
	suite.app.AccountKeeper.SetParams(infCtx, authtypes.DefaultParams())

	encodingConfig := encoding.MakeConfig(app.ModuleBasics)
	// We're using TestMsg amino encoding in some tests, so register it here.

	// nolint: exhaustruct
	encodingConfig.Amino.RegisterConcrete(&testdata.TestMsg{}, "testdata.TestMsg", nil)

	// nolint: exhaustruct
	suite.clientCtx = client.Context{}.WithTxConfig(encodingConfig.TxConfig)

	// nolint: exhaustruct
	anteHandler := ante.NewAnteHandler(suite.app.NewAnteHandlerOptions(simapp.EmptyAppOptions{}))

	suite.anteHandler = anteHandler

	// Also make a copy of the old Canto antehandler we were using to ensure that our changes fix the problem
	// nolint: exhaustruct
	oldAnteHandler := cantoante.NewAnteHandler(cantoante.HandlerOptions{
		AccountKeeper:   suite.app.AccountKeeper,
		BankKeeper:      suite.app.BankKeeper,
		EvmKeeper:       suite.app.EvmKeeper,
		FeegrantKeeper:  nil,
		IBCKeeper:       suite.app.IbcKeeper,
		FeeMarketKeeper: suite.app.FeemarketKeeper,
		SignModeHandler: encodingConfig.TxConfig.SignModeHandler(),
		SigGasConsumer:  althea.SigVerificationGasConsumer,
	})
	suite.oldAnteHandler = oldAnteHandler

	// Defines the siging method (e.g. homestead, london, etc)
	suite.ethSigner = ethtypes.LatestSignerForChainID(suite.app.EvmKeeper.ChainID())
}

// DefaultConsensusParams defines the default Tendermint consensus params used in
// EthermintApp testing.
// nolint: exhaustruct

func TestAnteTestSuite(t *testing.T) {
	// nolint: exhaustruct
	suite.Run(t, &AnteTestSuite{})
}

func (s *AnteTestSuite) NewEthermintPrivkey() *ethsecp256k1.PrivKey {
	privkey, err := ethsecp256k1.GenerateKey()
	if err != nil {
		return nil
	}
	_, err = privkey.ToECDSA()
	if err != nil {
		return nil
	}

	return privkey
}

func (s *AnteTestSuite) NewCosmosPrivkey() *secp256k1.PrivKey {
	return secp256k1.GenPrivKey()
}
func (s *AnteTestSuite) FundAccount(ctx sdk.Context, acc sdk.AccAddress, amount *big.Int) {
	address := common.BytesToAddress(acc.Bytes())
	s.Require().NoError(s.app.EvmKeeper.SetBalance(s.ctx, address, amount))
}

func (s *AnteTestSuite) BuildTestEVMTx(
	from common.Address,
	to common.Address,
	amount *big.Int,
	input []byte,
	gasPrice *big.Int,
	gasFeeCap *big.Int,
	gasTipCap *big.Int,
	accesses *ethtypes.AccessList,
) *evmtypes.MsgEthereumTx {
	chainID := s.app.EvmKeeper.ChainID()
	nonce := s.app.EvmKeeper.GetNonce(
		s.ctx,
		common.BytesToAddress(from.Bytes()),
	)

	msgEthereumTx := evmtypes.NewTx(
		chainID,
		nonce,
		&to,
		amount,
		TestGasLimit,
		gasPrice,
		gasFeeCap,
		gasTipCap,
		input,
		accesses,
	)
	msgEthereumTx.From = from.String()
	return msgEthereumTx
}

// CreateTestEVMTx is a helper function to create a tx given multiple inputs.
func (suite *AnteTestSuite) CreateTestEVMTx(
	msg *evmtypes.MsgEthereumTx, priv cryptotypes.PrivKey, accNum uint64, signCosmosTx bool,
	unsetExtensionOptions ...bool,
) authsigning.Tx {
	return suite.CreateTestEVMTxBuilder(msg, priv, accNum, signCosmosTx).GetTx()
}

// CreateTestEVMTxBuilder is a helper function to create a tx builder given multiple inputs.
func (suite *AnteTestSuite) CreateTestEVMTxBuilder(
	msg *evmtypes.MsgEthereumTx, priv cryptotypes.PrivKey, accNum uint64, signCosmosTx bool,
	unsetExtensionOptions ...bool,
) client.TxBuilder {
	var option *codectypes.Any
	var err error
	if len(unsetExtensionOptions) == 0 {
		option, err = codectypes.NewAnyWithValue(&evmtypes.ExtensionOptionsEthereumTx{})
		suite.Require().NoError(err)
	}

	txBuilder := suite.clientCtx.TxConfig.NewTxBuilder()
	builder, ok := txBuilder.(authtx.ExtensionOptionsTxBuilder)
	suite.Require().True(ok)

	if len(unsetExtensionOptions) == 0 {
		builder.SetExtensionOptions(option)
	}

	err = msg.Sign(suite.ethSigner, tests.NewSigner(priv))
	suite.Require().NoError(err)

	msg.From = ""
	err = builder.SetMsgs(msg)
	suite.Require().NoError(err)

	txData, err := evmtypes.UnpackTxData(msg.Data)
	suite.Require().NoError(err)

	fees := sdk.NewCoins(sdk.NewCoin(altheaconfig.BaseDenom, sdk.NewIntFromBigInt(txData.Fee())))
	builder.SetFeeAmount(fees)
	builder.SetGasLimit(msg.GetGas())

	if signCosmosTx {
		// First round: we gather all the signer infos. We use the "set empty
		// signature" hack to do that.
		sigV2 := signing.SignatureV2{
			PubKey: priv.PubKey(),
			Data: &signing.SingleSignatureData{
				SignMode:  suite.clientCtx.TxConfig.SignModeHandler().DefaultMode(),
				Signature: nil,
			},
			Sequence: txData.GetNonce(),
		}

		sigsV2 := []signing.SignatureV2{sigV2}

		err = txBuilder.SetSignatures(sigsV2...)
		suite.Require().NoError(err)

		// Second round: all signer infos are set, so each signer can sign.

		signerData := authsigning.SignerData{
			ChainID:       suite.ctx.ChainID(),
			AccountNumber: accNum,
			Sequence:      txData.GetNonce(),
		}
		sigV2, err = tx.SignWithPrivKey(
			suite.clientCtx.TxConfig.SignModeHandler().DefaultMode(), signerData,
			txBuilder, priv, suite.clientCtx.TxConfig, txData.GetNonce(),
		)
		suite.Require().NoError(err)

		sigsV2 = []signing.SignatureV2{sigV2}

		err = txBuilder.SetSignatures(sigsV2...)
		suite.Require().NoError(err)
	}

	return txBuilder
}

func (suite *AnteTestSuite) CreateTestCosmosMsgSend(gasPrice sdk.Int, denom string, amount sdk.Int, from sdk.AccAddress, to sdk.AccAddress) client.TxBuilder {
	return suite.CreateTestCosmosTxBuilder(gasPrice, denom, banktypes.NewMsgSend(from, to, sdk.NewCoins(sdk.NewCoin(denom, amount))))
}

func (suite *AnteTestSuite) CreateTestCosmosMsgMicrotx(gasPrice sdk.Int, denom string, amount sdk.Int, from sdk.AccAddress, to sdk.AccAddress) client.TxBuilder {
	return suite.CreateTestCosmosTxBuilder(gasPrice, denom, microtxtypes.NewMsgMicrotx(from.String(), to.String(), sdk.NewCoin(denom, amount)))
}

func (suite *AnteTestSuite) CreateTestCosmosMsgMicrotxMsgSend(gasPrice sdk.Int, denom string, amount sdk.Int, from sdk.AccAddress, to sdk.AccAddress) client.TxBuilder {
	msgMicrotx := microtxtypes.NewMsgMicrotx(from.String(), to.String(), sdk.NewCoin(denom, amount))
	msgSend := banktypes.NewMsgSend(from, to, sdk.NewCoins(sdk.NewCoin(denom, amount)))
	return suite.CreateTestCosmosTxBuilder(gasPrice, denom, msgMicrotx, msgSend)
}

func (suite *AnteTestSuite) CreateTestCosmosTxBuilder(gasPrice sdk.Int, denom string, msgs ...sdk.Msg) client.TxBuilder {
	txBuilder := suite.clientCtx.TxConfig.NewTxBuilder()

	txBuilder.SetGasLimit(TestGasLimit)
	fees := &sdk.Coins{{Denom: denom, Amount: gasPrice.MulRaw(int64(TestGasLimit))}}
	txBuilder.SetFeeAmount(*fees)
	err := txBuilder.SetMsgs(msgs...)
	suite.Require().NoError(err)
	return txBuilder
}

func (suite *AnteTestSuite) SignTestCosmosTx(chainId string, txBuilder client.TxBuilder, privkey cryptotypes.PrivKey, accNum uint64, sequence uint64) client.TxBuilder {
	signMode := suite.clientCtx.TxConfig.SignModeHandler().DefaultMode()
	pubKey := privkey.PubKey()
	signerData := authsigning.SignerData{
		ChainID:       chainId,
		AccountNumber: accNum,
		Sequence:      sequence,
	}

	sigData := signing.SingleSignatureData{
		SignMode:  signMode,
		Signature: nil,
	}
	sig := signing.SignatureV2{
		PubKey:   pubKey,
		Data:     &sigData,
		Sequence: sequence,
	}
	if err := txBuilder.SetSignatures(sig); err != nil {
		panic("Unable to set signatures: " + err.Error())
	}

	// Generate the bytes to be signed.
	bytesToSign, err := suite.clientCtx.TxConfig.SignModeHandler().GetSignBytes(signMode, signerData, txBuilder.GetTx())
	if err != nil {
		panic("Unable to get sign bytes: " + err.Error())
	}

	// Sign those bytes
	sigBytes, err := privkey.Sign(bytesToSign)
	if err != nil {
		panic("Unable to sign: " + err.Error())
	}

	// Construct the SignatureV2 struct
	sigData = signing.SingleSignatureData{
		SignMode:  signMode,
		Signature: sigBytes,
	}
	sig = signing.SignatureV2{
		PubKey:   pubKey,
		Data:     &sigData,
		Sequence: sequence,
	}

	suite.Require().NoError(txBuilder.SetSignatures(sig))
	return txBuilder
}

func (suite *AnteTestSuite) CreateSignedCosmosTx(ctx sdk.Context, txBuilder client.TxBuilder, priv cryptotypes.PrivKey) sdk.Tx {
	addr := sdk.AccAddress(priv.PubKey().Address().Bytes())
	acc := suite.app.AccountKeeper.GetAccount(ctx, addr)
	return suite.SignTestCosmosTx(suite.ctx.ChainID(), txBuilder, priv, acc.GetAccountNumber(), acc.GetSequence()).GetTx()
}

func (suite *AnteTestSuite) CreateTestEIP712TxBuilderMsgSend(from sdk.AccAddress, priv cryptotypes.PrivKey, chainId string, gas uint64, gasAmount sdk.Coins) client.TxBuilder {
	// Build MsgSend
	recipient := sdk.AccAddress(common.Address{}.Bytes())
	msgSend := banktypes.NewMsgSend(from, recipient, sdk.NewCoins(sdk.NewCoin(altheaconfig.BaseDenom, sdk.NewInt(1))))
	return suite.CreateTestEIP712CosmosTxBuilder(from, priv, chainId, gas, gasAmount, msgSend)
}

func (suite *AnteTestSuite) CreateTestEIP712TxBuilderMsgDelegate(from sdk.AccAddress, priv cryptotypes.PrivKey, chainId string, gas uint64, gasAmount sdk.Coins) client.TxBuilder {
	// Build MsgSend
	valEthAddr := tests.GenerateAddress()
	valAddr := sdk.ValAddress(valEthAddr.Bytes())
	msgSend := stakingtypes.NewMsgDelegate(from, valAddr, sdk.NewCoin(altheaconfig.BaseDenom, sdk.NewInt(20)))
	return suite.CreateTestEIP712CosmosTxBuilder(from, priv, chainId, gas, gasAmount, msgSend)
}

func (suite *AnteTestSuite) CreateTestEIP712CosmosTxBuilder(
	from sdk.AccAddress, priv cryptotypes.PrivKey, chainId string, gas uint64, gasAmount sdk.Coins, msg sdk.Msg,
) client.TxBuilder {
	var err error

	nonce, err := suite.app.AccountKeeper.GetSequence(suite.ctx, from)
	suite.Require().NoError(err)

	pc, err := types.ParseChainID(chainId)
	suite.Require().NoError(err)
	ethChainId := pc.Uint64()

	// GenerateTypedData TypedData
	var ethermintCodec codec.ProtoCodecMarshaler
	// nolint: staticcheck
	fee := legacytx.NewStdFee(gas, gasAmount)
	accNumber := suite.app.AccountKeeper.GetAccount(suite.ctx, from).GetAccountNumber()

	data := legacytx.StdSignBytes(chainId, accNumber, nonce, 0, fee, []sdk.Msg{msg}, "")
	typedData, err := eip712.WrapTxToTypedData(ethermintCodec, ethChainId, msg, data, &eip712.FeeDelegationOptions{
		FeePayer: from,
	})
	suite.Require().NoError(err)

	sigHash, err := eip712.ComputeTypedDataHash(typedData)
	suite.Require().NoError(err)

	// Sign typedData
	keyringSigner := tests.NewSigner(priv)
	signature, pubKey, err := keyringSigner.SignByAddress(from, sigHash)
	suite.Require().NoError(err)
	signature[crypto.RecoveryIDOffset] += 27 // Transform V from 0/1 to 27/28 according to the yellow paper

	// Add ExtensionOptionsWeb3Tx extension
	var option *codectypes.Any
	option, err = codectypes.NewAnyWithValue(&types.ExtensionOptionsWeb3Tx{
		FeePayer:         from.String(),
		TypedDataChainID: ethChainId,
		FeePayerSig:      signature,
	})
	suite.Require().NoError(err)

	suite.clientCtx.TxConfig.SignModeHandler()
	txBuilder := suite.clientCtx.TxConfig.NewTxBuilder()
	builder, ok := txBuilder.(authtx.ExtensionOptionsTxBuilder)
	suite.Require().True(ok)

	builder.SetExtensionOptions(option)
	builder.SetFeeAmount(gasAmount)
	builder.SetGasLimit(gas)

	sigsV2 := signing.SignatureV2{
		PubKey: pubKey,
		Data: &signing.SingleSignatureData{
			SignMode:  signing.SignMode_SIGN_MODE_LEGACY_AMINO_JSON,
			Signature: []byte{},
		},
		Sequence: nonce,
	}

	err = builder.SetSignatures(sigsV2)
	suite.Require().NoError(err)

	err = builder.SetMsgs(msg)
	suite.Require().NoError(err)

	return builder
}

func NextFn(ctx sdk.Context, _ sdk.Tx, _ bool) (sdk.Context, error) {
	return ctx, nil
}

var _ sdk.Tx = &invalidTx{}

type invalidTx struct{}

func (invalidTx) GetMsgs() []sdk.Msg   { return []sdk.Msg{nil} }
func (invalidTx) ValidateBasic() error { return nil }
