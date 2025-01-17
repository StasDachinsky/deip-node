use frame_support::{
    dispatch::DispatchResult,
    traits::{
        fungibles::{Inspect, Mutate},
        tokens::{DepositConsequence, WithdrawConsequence},
    },
};
use frame_system::Config as SystemConfig;
use sp_runtime::DispatchError;

use crate::{Config, Pallet};

impl<T: Config> Inspect<<T as SystemConfig>::AccountId> for Pallet<T> {
    type AssetId = <T as pallet_assets::Config>::AssetId;

    type Balance = <T as pallet_assets::Config>::Balance;

    fn total_issuance(asset: Self::AssetId) -> Self::Balance {
        pallet_assets::Pallet::<T>::total_issuance(asset)
    }

    fn minimum_balance(asset: Self::AssetId) -> Self::Balance {
        pallet_assets::Pallet::<T>::minimum_balance(asset)
    }

    fn balance(asset: Self::AssetId, who: &<T as SystemConfig>::AccountId) -> Self::Balance {
        pallet_assets::Pallet::<T>::balance(asset, who)
    }

    fn reducible_balance(
        asset: Self::AssetId,
        who: &<T as SystemConfig>::AccountId,
        keep_alive: bool,
    ) -> Self::Balance {
        pallet_assets::Pallet::<T>::reducible_balance(asset, who, keep_alive)
    }

    fn can_deposit(
        asset: Self::AssetId,
        who: &<T as SystemConfig>::AccountId,
        amount: Self::Balance,
    ) -> DepositConsequence {
        pallet_assets::Pallet::<T>::can_deposit(asset, who, amount)
    }

    fn can_withdraw(
        asset: Self::AssetId,
        who: &<T as SystemConfig>::AccountId,
        amount: Self::Balance,
    ) -> WithdrawConsequence<Self::Balance> {
        pallet_assets::Pallet::<T>::can_withdraw(asset, who, amount)
    }
}

impl<T: Config> Mutate<<T as SystemConfig>::AccountId> for Pallet<T> {
    fn mint_into(
        asset: Self::AssetId,
        who: &<T as SystemConfig>::AccountId,
        amount: Self::Balance,
    ) -> DispatchResult {
        pallet_assets::Pallet::<T>::mint_into(asset, who, amount)
    }

    fn burn_from(
        asset: Self::AssetId,
        who: &<T as SystemConfig>::AccountId,
        amount: Self::Balance,
    ) -> Result<Self::Balance, DispatchError> {
        pallet_assets::Pallet::<T>::burn_from(asset, who, amount)
    }
}
