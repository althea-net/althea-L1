package types

import (
	"fmt"

	paramtypes "github.com/cosmos/cosmos-sdk/x/params/types"
)

// Parameter store key
var (
	ParamStoreKeyEnableOnboarding     = []byte("EnableOnboarding")
	ParamsStoreKeyWhitelistedChannels = []byte("WhitelistedChannels")
	DefaultWhitelistedChannels        = []string{}
)

var _ paramtypes.ParamSet = &Params{}

// ParamKeyTable returns the parameter key table.
func ParamKeyTable() paramtypes.KeyTable {
	return paramtypes.NewKeyTable().RegisterParamSet(&Params{})
}

// NewParams creates a new Params instance
func NewParams(
	enableOnboarding bool,
	whitelistedChannels []string,
) Params {
	return Params{
		EnableOnboarding:    enableOnboarding,
		WhitelistedChannels: whitelistedChannels,
	}
}

// DefaultParams defines the default params for the onboarding module
func DefaultParams() Params {
	return Params{
		EnableOnboarding:    true,
		WhitelistedChannels: DefaultWhitelistedChannels,
	}
}

// ParamSetPairs returns the parameter set pairs.
func (p *Params) ParamSetPairs() paramtypes.ParamSetPairs {
	return paramtypes.ParamSetPairs{
		paramtypes.NewParamSetPair(ParamStoreKeyEnableOnboarding, &p.EnableOnboarding, validateBool),
		paramtypes.NewParamSetPair(ParamsStoreKeyWhitelistedChannels, &p.WhitelistedChannels, validateWhitelistedChannels),
	}
}

func validateWhitelistedChannels(i interface{}) error {
	_, ok := i.([]string)
	if !ok {
		return fmt.Errorf("invalid parameter type: %T", i)
	}

	return nil
}

func validateBool(i interface{}) error {
	_, ok := i.(bool)
	if !ok {
		return fmt.Errorf("invalid parameter type: %T", i)
	}

	return nil
}

// Validate checks that the fields have valid values
func (p Params) Validate() error {
	if err := validateBool(p.EnableOnboarding); err != nil {
		return err
	}
	return nil
}
