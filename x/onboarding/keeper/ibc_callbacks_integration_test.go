package keeper_test

import (
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	sdk "github.com/cosmos/cosmos-sdk/types"

	althea "github.com/althea-net/althea-L1/app"
	onboardingtypes "github.com/althea-net/althea-L1/x/onboarding/types"
)

var _ = Describe("Onboarding: Performing an IBC Transfer followed by autoswap and convert", Ordered, func() {
	coinAlthea := sdk.NewCoin("aalthea", sdk.ZeroInt())
	ibcBalance := sdk.NewCoin(uusdcIbcdenom, sdk.NewIntWithDecimal(10000, 6))
	coinAtom := sdk.NewCoin("uatom", sdk.NewIntWithDecimal(10000, 6))

	var (
		sender, receiver string
		senderAcc        sdk.AccAddress
		receiverAcc      sdk.AccAddress
	)

	BeforeEach(func() {
		s.SetupTest()
	})

	Describe("from a non-authorized channel: Cosmos ---(uatom)---> Althea", func() {
		BeforeEach(func() {
			// send coins from Cosmos to Althea
			sender = s.IBCCosmosChain.SenderAccount.GetAddress().String()
			receiver = s.AltheaChain.SenderAccount.GetAddress().String()
			senderAcc = sdk.MustAccAddressFromBech32(sender)
			receiverAcc = sdk.MustAccAddressFromBech32(receiver)
			s.SendAndReceiveMessage(s.PathCosmosAlthea, s.IBCCosmosChain, "uatom", 10000000000, sender, receiver, 1)

		})
		It("No convert operation - aalthea balance should be 0", func() {
			nativeAlthea := s.AltheaChain.App.(*althea.AltheaApp).BankKeeper.GetBalance(s.AltheaChain.GetContext(), receiverAcc, "aalthea")
			Expect(nativeAlthea).To(Equal(coinAlthea))
		})
		It("Althea chain's IBC voucher balance should be same with the transferred amount", func() {
			ibcAtom := s.AltheaChain.App.(*althea.AltheaApp).BankKeeper.GetBalance(s.AltheaChain.GetContext(), receiverAcc, uatomIbcdenom)
			Expect(ibcAtom).To(Equal(sdk.NewCoin(uatomIbcdenom, coinAtom.Amount)))
		})
		It("Cosmos chain's uatom balance should be 0", func() {
			atom := s.IBCCosmosChain.GetSimApp().BankKeeper.GetBalance(s.IBCCosmosChain.GetContext(), senderAcc, "uatom")
			Expect(atom).To(Equal(sdk.NewCoin("uatom", sdk.ZeroInt())))
		})
	})

	Describe("from an authorized channel: Gravity ---(uUSDC)---> Althea", func() {
		When("ERC20 contract is deployed and token pair is enabled", func() {
			BeforeEach(func() {
				sender = s.IBCGravityChain.SenderAccount.GetAddress().String()
				receiver = s.AltheaChain.SenderAccount.GetAddress().String()
				senderAcc = sdk.MustAccAddressFromBech32(sender)
				receiverAcc = sdk.MustAccAddressFromBech32(receiver)

				s.FundAltheaChain(sdk.NewCoins(ibcBalance))

			})

			When("ERC20 contract is not deployed", func() {
				BeforeEach(func() {
					s.AltheaChain.App.(*althea.AltheaApp).OnboardingKeeper.SetParams(s.AltheaChain.GetContext(), onboardingtypes.Params{EnableOnboarding: true, WhitelistedChannels: []string{s.PathGravityAlthea.EndpointB.ChannelID}})
					sender = s.IBCGravityChain.SenderAccount.GetAddress().String()
					receiver = s.AltheaChain.SenderAccount.GetAddress().String()
					senderAcc = sdk.MustAccAddressFromBech32(sender)
					receiverAcc = sdk.MustAccAddressFromBech32(receiver)

					s.FundAltheaChain(sdk.NewCoins(sdk.NewCoin("aalthea", sdk.NewIntWithDecimal(3, 18))))
					s.SendAndReceiveMessage(s.PathGravityAlthea, s.IBCGravityChain, "uUSDC", 10000000000, sender, receiver, 1)
				})
				It("No convert: Althea chain's IBC voucher balance should be same with (original balance + transferred amount)", func() {
					ibcUsdc := s.AltheaChain.App.(*althea.AltheaApp).BankKeeper.GetBalance(s.AltheaChain.GetContext(), receiverAcc, uusdcIbcdenom)
					Expect(ibcUsdc.Amount).To(Equal(ibcBalance.Amount.Add(sdk.NewInt(10000000000))))
				})

			})
		})
	})
})
