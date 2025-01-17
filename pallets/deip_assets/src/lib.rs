//! # DEIP Assets Module
//! A module provides functionality of User Issued Assets.
//!
//! - [`Config`](./trait.Config.html)
//!
//! ## Overview
//! The pallet wraps Substrate [`pallet_assets`](../pallet_assets/index.html) and
//! adds additional constraints/features.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! * [`create_asset`](./enum.Call.html#variant.create_asset)
//! * [`destroy`](./enum.Call.html#variant.destroy)
//! * [`issue_asset`](./enum.Call.html#variant.issue_asset)
//! * [`burn`](./enum.Call.html#variant.burn)
//! * [`transfer`](./enum.Call.html#variant.transfer)
//! * [`freeze`](./enum.Call.html#variant.freeze)
//! * [`thaw`](./enum.Call.html#variant.thaw)
//! * [`freeze_asset`](./enum.Call.html#variant.freeze_asset)
//! * [`thaw_asset`](./enum.Call.html#variant.thaw_asset)
//! * [`transfer_ownership`](./enum.Call.html#variant.transfer_ownership)
//! * [`set_team`](./enum.Call.html#variant.set_team)
//! * [`set_metadata`](./enum.Call.html#variant.set_metadata)
//!
//! [`Config`]: ./trait.Config.html

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

// Re-export to use implementation details in dependent crates:
pub use pallet_assets;

mod impl_fungibles;

pub use deip_serializable_u128::SerializableAtLeast32BitUnsigned as SerializableAssetBalance;

#[doc(inline)]
pub use pallet::*;

const NON_LOCAL: u8 = 101;

#[frame_support::pallet]
#[doc(hidden)]
pub mod pallet {
    use frame_support::{
        pallet_prelude::{
            ensure, Blake2_128Concat, Decode, DispatchResult, DispatchResultWithPostInfo, Encode,
            Get, Hooks, Identity, InvalidTransaction, MaxEncodedLen, Member, OptionQuery,
            Parameter, Pays, StorageDoubleMap, StorageMap, StorageValue, TransactionSource,
            TransactionValidity, ValidTransaction, ValidateUnsigned, ValueQuery,
        },
        traits::{Currency, ExistenceRequirement, UnfilteredDispatchable, WithdrawReasons},
        transactional, RuntimeDebug,
    };
    use frame_system::{
        offchain::{SendTransactionTypes, SubmitTransaction},
        pallet_prelude::{ensure_none, ensure_signed, BlockNumberFor, OriginFor},
        RawOrigin,
    };
    use scale_info::TypeInfo;
    use sp_runtime::traits::{CheckedAdd, One, StaticLookup, Zero};
    use sp_std::{
        prelude::{Clone, Vec},
        vec,
    };

    use codec::HasCompact;

    #[cfg(feature = "std")]
    use frame_support::traits::GenesisBuild;

    use pallet_assets::{DestroyWitness, WeightInfo};

    use deip_asset_system::AssetIdInitT;
    use deip_projects_info::DeipProjectsInfo;

    type AccountIdOf<T> = <T as frame_system::Config>::AccountId;
    pub type ProjectsInfoOf<T> = <T as Config>::ProjectsInfo;
    pub type DeipProjectIdOf<T> =
        <<T as Config>::ProjectsInfo as DeipProjectsInfo<AccountIdOf<T>>>::ProjectId;
    type DeipInvestmentIdOf<T> =
        <<T as Config>::ProjectsInfo as DeipProjectsInfo<AccountIdOf<T>>>::InvestmentId;
    pub(crate) type AssetsAssetIdOf<T> = <T as Config>::AssetsAssetId;
    pub(crate) type AssetsBalanceOf<T> = <T as pallet_assets::Config>::Balance;
    pub type DeipAssetIdOf<T> = <T as Config>::AssetId;
    type AssetsWeightInfoOf<T> = <T as pallet_assets::Config>::WeightInfo;

    // pub trait AssetIdInitT<T: Config>: TypeInfo {
    //     fn from_bytes(source: &[u8]) -> <T as Config>::AssetId;
    // }

    #[pallet::config]
    pub trait Config:
        frame_system::Config
        + pallet_assets::Config<AssetId = Self::AssetsAssetId>
        + SendTransactionTypes<Call<Self>>
    {
        type ProjectsInfo: DeipProjectsInfo<Self::AccountId>;
        type DeipAccountId: Into<Self::AccountId> + From<Self::AccountId> + Parameter + Member;

        type AssetsAssetId: Member
            + Parameter
            + Default
            + Copy
            + HasCompact
            + MaxEncodedLen
            + CheckedAdd
            + One;

        #[cfg(not(feature = "std"))]
        type AssetId: Member + Parameter + Default + Copy;
        #[cfg(feature = "std")]
        type AssetId: Member
            + Parameter
            + Default
            + Copy
            + serde::Serialize
            + serde::de::DeserializeOwned
            + TypeInfo;
        type AssetIdInit: AssetIdInitT<<Self as Config>::AssetId>;

        /// Period of check for accounts with zero FTs
        #[pallet::constant]
        type WipePeriod: Get<Self::BlockNumber>;
    }

    #[doc(hidden)]
    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    #[doc(hidden)]
    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn offchain_worker(n: T::BlockNumber) {
            if !sp_io::offchain::is_validator() {
                return
            }

            if n % T::WipePeriod::get() != Zero::zero() {
                return
            }

            for (asset, accounts) in FtBalanceMap::<T>::iter() {
                for account in accounts {
                    if !Self::account_balance(&account, &asset).is_zero() {
                        continue
                    }

                    let call = Call::deip_wipe_zero_balance { asset, account };
                    let _submit =
                        SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into());
                }
            }
        }
    }

    #[pallet::validate_unsigned]
    impl<T: Config> ValidateUnsigned for Pallet<T> {
        type Call = Call<T>;

        fn validate_unsigned(source: TransactionSource, call: &Self::Call) -> TransactionValidity {
            if !matches!(source, TransactionSource::Local | TransactionSource::InBlock) {
                return InvalidTransaction::Custom(super::NON_LOCAL).into()
            }

            if let Call::deip_wipe_zero_balance { asset, account } = &call {
                if !Self::account_balance(account, asset).is_zero() {
                    return InvalidTransaction::Stale.into()
                }

                let balances = match FtBalanceMap::<T>::try_get(*asset) {
                    Err(_) => return InvalidTransaction::Stale.into(),
                    Ok(b) => b,
                };

                if balances.binary_search_by_key(&account, |a| a).is_err() {
                    return InvalidTransaction::Stale.into()
                }

                ValidTransaction::with_tag_prefix("DeipAssetsOffchainWorker")
                    .propagate(false)
                    .longevity(5)
                    .and_provides((*asset, account.clone()))
                    .build()
            } else {
                InvalidTransaction::Call.into()
            }
        }
    }

    #[pallet::error]
    pub enum Error<T> {
        ProjectDoesNotExist,
        ProjectDoesNotBelongToTeam,
        ProjectSecurityTokenCannotBeDestroyed,
        ProjectSecurityTokenCannotBeBurned,
        ProjectSecurityTokenCannotBeFreezed,
        ProjectSecurityTokenAccountCannotBeFreezed,
        ReservedAssetCannotBeFreezed,
        ReservedAssetAccountCannotBeFreezed,
        FtNotFound,
        FtBalanceNotFound,
        AssetIdOverflow,
        DeipAssetIdExists,
        /// Asset with DeipAssetId wasn't created.
        DeipAssetIdDoesNotExist,
    }

    #[pallet::storage]
    pub(super) type AssetIdByDeipAssetId<T: Config> = StorageDoubleMap<
        _,
        Identity,
        DeipAssetIdOf<T>,
        Blake2_128Concat,
        AssetsAssetIdOf<T>,
        (),
        OptionQuery,
    >;

    #[pallet::storage]
    pub(super) type DeipAssetIdByAssetId<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        AssetsAssetIdOf<T>,
        Identity,
        DeipAssetIdOf<T>,
        (),
        OptionQuery,
    >;

    #[pallet::storage]
    pub(super) type NextAssetId<T> = StorageValue<_, AssetsAssetIdOf<T>, ValueQuery>;

    #[pallet::storage]
    pub(super) type AssetIdByProjectId<T: Config> =
        StorageMap<_, Identity, DeipProjectIdOf<T>, Vec<DeipAssetIdOf<T>>, OptionQuery>;

    #[pallet::storage]
    pub(super) type ProjectIdByAssetId<T: Config> =
        StorageMap<_, Identity, DeipAssetIdOf<T>, DeipProjectIdOf<T>, OptionQuery>;

    #[pallet::storage]
    pub(super) type InvestmentByAssetId<T: Config> =
        StorageMap<_, Identity, DeipAssetIdOf<T>, Vec<DeipInvestmentIdOf<T>>, OptionQuery>;

    #[derive(Encode, Decode, Clone, Default, RuntimeDebug, PartialEq, Eq, TypeInfo)]
    pub(super) struct Investment<AccountId, AssetId> {
        creator: AccountId,
        assets: Vec<AssetId>,
        asset_id: AssetId,
    }

    #[pallet::storage]
    pub(super) type InvestmentMap<T: Config> = StorageMap<
        _,
        Identity,
        DeipInvestmentIdOf<T>,
        Investment<AccountIdOf<T>, DeipAssetIdOf<T>>,
        OptionQuery,
    >;

    #[pallet::storage]
    pub(super) type FtBalanceMap<T: Config> =
        StorageMap<_, Identity, DeipAssetIdOf<T>, Vec<AccountIdOf<T>>, OptionQuery>;

    #[derive(Encode, Decode, Clone, Default, RuntimeDebug, PartialEq, Eq, TypeInfo)]
    pub(super) struct AssetMetadata<U8> {
        name: Vec<U8>,
        symbol: Vec<U8>,
        decimals: U8,
    }

    #[pallet::storage]
    pub(super) type AssetMetadataMap<T: Config> =
        StorageMap<_, Identity, DeipAssetIdOf<T>, AssetMetadata<u8>, OptionQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T> {
        pub _marker: std::marker::PhantomData<T>,
    }

    #[cfg(feature = "std")]
    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self { _marker: std::marker::PhantomData }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
        fn build(&self) {}
    }

    impl<T: Config> Pallet<T> {
        pub fn project_key(id: &DeipProjectIdOf<T>) -> T::AccountId {
            let entropy =
                (b"deip/projects/", id.as_ref()).using_encoded(sp_io::hashing::blake2_256);
            T::AccountId::decode(&mut &entropy[..]).unwrap_or_default()
        }

        pub fn investment_key(id: &DeipInvestmentIdOf<T>) -> T::AccountId {
            let entropy =
                (b"deip/investments/", id.as_ref()).using_encoded(sp_io::hashing::blake2_256);
            T::AccountId::decode(&mut &entropy[..]).unwrap_or_default()
        }

        pub fn try_get_tokenized_project(id: &DeipAssetIdOf<T>) -> Option<DeipProjectIdOf<T>> {
            match ProjectIdByAssetId::<T>::try_get(*id) {
                Ok(project_id) => Some(project_id),
                Err(_) => None,
            }
        }

        pub fn account_balance(account: &AccountIdOf<T>, asset: &DeipAssetIdOf<T>) -> T::Balance {
            match AssetIdByDeipAssetId::<T>::iter_prefix(*asset).next() {
                None => Default::default(),
                Some(prefix) => pallet_assets::Pallet::<T>::balance(prefix.0, account.clone()),
            }
        }

        pub fn total_supply(asset: &DeipAssetIdOf<T>) -> T::Balance {
            match AssetIdByDeipAssetId::<T>::iter_prefix(*asset).next() {
                None => Zero::zero(),
                Some(prefix) => pallet_assets::Pallet::<T>::total_supply(prefix.0),
            }
        }

        pub fn get_project_fts(id: &DeipProjectIdOf<T>) -> Vec<DeipAssetIdOf<T>> {
            AssetIdByProjectId::<T>::try_get(id.clone()).unwrap_or_default()
        }

        pub fn get_ft_balances(id: &DeipAssetIdOf<T>) -> Option<Vec<AccountIdOf<T>>> {
            FtBalanceMap::<T>::try_get(*id).ok()
        }

        #[transactional]
        pub fn transactionally_transfer(
            from: &AccountIdOf<T>,
            asset: DeipAssetIdOf<T>,
            transfers: &[(AssetsBalanceOf<T>, AccountIdOf<T>)],
        ) -> Result<(), ()> {
            for (amount, to) in transfers {
                let result = Self::deip_transfer_impl(
                    RawOrigin::Signed(from.clone()).into(),
                    asset,
                    to.clone(),
                    *amount,
                );
                if result.is_err() {
                    return Err(())
                }
            }

            Ok(())
        }

        #[transactional]
        pub fn deip_transactionally_reserve(
            account: &T::AccountId,
            id: DeipInvestmentIdOf<T>,
            shares: &[(DeipAssetIdOf<T>, AssetsBalanceOf<T>)],
            asset_to_raise: DeipAssetIdOf<T>,
        ) -> Result<(), deip_assets_error::ReserveError<DeipAssetIdOf<T>>> {
            use deip_assets_error::ReserveError;

            ensure!(!InvestmentMap::<T>::contains_key(id.clone()), ReserveError::AlreadyReserved);

            let id_account = Self::investment_key(&id);
            let id_source = <T::Lookup as StaticLookup>::unlookup(id_account.clone());

            let reserved = T::Currency::withdraw(
                account,
                T::Currency::minimum_balance(),
                WithdrawReasons::RESERVE,
                ExistenceRequirement::AllowDeath,
            )
            .map_err(|_| ReserveError::NotEnoughBalance)?;

            T::Currency::resolve_creating(&id_account, reserved);

            let mut assets_to_reserve = Vec::<DeipAssetIdOf<T>>::with_capacity(shares.len());

            for (asset, amount) in shares {
                let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(asset)
                    .next()
                    .ok_or(ReserveError::AssetTransferFailed(*asset))?
                    .0;
                let call = pallet_assets::Call::<T>::transfer {
                    id: asset_id,
                    target: id_source.clone(),
                    amount: *amount,
                };
                let result = call.dispatch_bypass_filter(RawOrigin::Signed(account.clone()).into());
                if result.is_err() {
                    return Err(ReserveError::AssetTransferFailed(*asset))
                }

                assets_to_reserve.push(*asset);

                InvestmentByAssetId::<T>::mutate_exists(*asset, |investments| {
                    match investments.as_mut() {
                        None => *investments = Some(vec![id.clone()]),
                        Some(c) => c.push(id.clone()),
                    };
                });
            }

            InvestmentByAssetId::<T>::mutate_exists(asset_to_raise, |investments| {
                match investments.as_mut() {
                    None => *investments = Some(vec![id.clone()]),
                    Some(c) => c.push(id.clone()),
                };
            });

            InvestmentMap::<T>::insert(
                id.clone(),
                Investment {
                    creator: account.clone(),
                    assets: assets_to_reserve,
                    asset_id: asset_to_raise,
                },
            );

            Ok(())
        }

        #[transactional]
        pub fn transactionally_unreserve(
            id: DeipInvestmentIdOf<T>,
        ) -> Result<(), deip_assets_error::UnreserveError<DeipAssetIdOf<T>>> {
            use deip_assets_error::UnreserveError;

            let info = match InvestmentMap::<T>::take(id.clone()) {
                Some(i) => i,
                None => return Err(UnreserveError::NoSuchInvestment),
            };

            let deposited =
                T::Currency::deposit_creating(&info.creator, T::Currency::minimum_balance());

            let id_account = Self::investment_key(&id);

            for asset_id in info.assets.iter().chain(&[info.asset_id]) {
                InvestmentByAssetId::<T>::mutate_exists(*asset_id, |maybe_investments| {
                    let investments =
                        maybe_investments.as_mut().expect("checked in transactionally_reserve");
                    let index = investments
                        .iter()
                        .position(|a| *a == id)
                        .expect("checked in transactionally_reserve");
                    investments.remove(index);
                    if investments.is_empty() {
                        *maybe_investments = None;
                    }
                });

                let amount = Self::account_balance(&id_account, asset_id);
                if amount.is_zero() {
                    continue
                }

                let result = Self::deip_transfer_impl(
                    RawOrigin::Signed(id_account.clone()).into(),
                    *asset_id,
                    info.creator.clone(),
                    amount,
                );
                if result.is_err() {
                    return Err(UnreserveError::AssetTransferFailed(*asset_id))
                }
            }

            T::Currency::settle(
                &id_account,
                deposited,
                WithdrawReasons::TRANSFER,
                ExistenceRequirement::AllowDeath,
            )
            .unwrap_or_else(|_| panic!("should be reserved in transactionally_reserve"));

            Ok(())
        }

        pub fn transfer_from_reserved(
            id: DeipInvestmentIdOf<T>,
            who: &T::AccountId,
            asset: DeipAssetIdOf<T>,
            amount: AssetsBalanceOf<T>,
        ) -> Result<(), deip_assets_error::UnreserveError<DeipAssetIdOf<T>>> {
            use deip_assets_error::UnreserveError;

            ensure!(InvestmentMap::<T>::contains_key(id.clone()), UnreserveError::NoSuchInvestment);

            let id_account = Self::investment_key(&id);

            let result = Self::deip_transfer_impl(
                RawOrigin::Signed(id_account).into(),
                asset,
                who.clone(),
                amount,
            );
            if result.is_err() {
                return Err(UnreserveError::AssetTransferFailed(asset))
            }

            Ok(())
        }

        pub fn deip_transfer_to_reserved(
            who: &T::AccountId,
            id: DeipInvestmentIdOf<T>,
            amount: AssetsBalanceOf<T>,
        ) -> Result<(), deip_assets_error::UnreserveError<DeipAssetIdOf<T>>> {
            use deip_assets_error::UnreserveError;

            let info = match InvestmentMap::<T>::try_get(id.clone()) {
                Ok(i) => i,
                Err(_) => return Err(UnreserveError::NoSuchInvestment),
            };

            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(info.asset_id)
                .next()
                .ok_or(UnreserveError::AssetTransferFailed(info.asset_id))?
                .0;

            let id_account = Self::investment_key(&id);
            let id_source = <T::Lookup as StaticLookup>::unlookup(id_account);

            let call =
                pallet_assets::Call::<T>::transfer { id: asset_id, target: id_source, amount };
            let result = call.dispatch_bypass_filter(RawOrigin::Signed(who.clone()).into());
            if result.is_err() {
                return Err(UnreserveError::AssetTransferFailed(info.asset_id))
            }

            Ok(())
        }

        // stores `to` in the map of FT-balances if the asset tokenizes some active
        fn deip_transfer_impl(
            from: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            to: AccountIdOf<T>,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResultWithPostInfo {
            let target_source = <T::Lookup as StaticLookup>::unlookup(to.clone());
            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call =
                pallet_assets::Call::<T>::transfer { id: asset_id, target: target_source, amount };
            let ok = call.dispatch_bypass_filter(from)?;

            if Self::try_get_tokenized_project(&id).is_some() {
                FtBalanceMap::<T>::mutate_exists(id, |maybe| match maybe.as_mut() {
                    None => {
                        // this cannot happen but for any case
                        *maybe = Some(vec![to]);
                    },
                    Some(b) => match b.binary_search_by_key(&&to, |a| a) {
                        Ok(_) => (),
                        Err(i) => b.insert(i, to),
                    },
                });
            }

            Ok(ok)
        }

        fn deip_create_asset_impl(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            admin: T::AccountId,
            min_balance: AssetsBalanceOf<T>,
            project_id: Option<DeipProjectIdOf<T>>,
        ) -> DispatchResultWithPostInfo {
            if let Some(ref id) = project_id {
                match T::ProjectsInfo::try_get_project_team(id) {
                    None => return Err(Error::<T>::ProjectDoesNotExist.into()),
                    Some(team_id) => {
                        let account = ensure_signed(origin.clone())?;
                        ensure!(team_id == account, Error::<T>::ProjectDoesNotBelongToTeam)
                    },
                };
            }

            ensure!(
                AssetIdByDeipAssetId::<T>::iter_prefix(id).next().is_none(),
                Error::<T>::DeipAssetIdExists
            );

            let asset_id = NextAssetId::<T>::get();
            let next_asset_id =
                asset_id.checked_add(&One::one()).ok_or(Error::<T>::AssetIdOverflow)?;

            let admin_source = <T::Lookup as StaticLookup>::unlookup(admin);
            let call =
                pallet_assets::Call::<T>::create { id: asset_id, admin: admin_source, min_balance };
            let post_dispatch_info = call.dispatch_bypass_filter(origin)?;

            NextAssetId::<T>::put(next_asset_id);
            AssetIdByDeipAssetId::<T>::insert(id, asset_id, ());
            DeipAssetIdByAssetId::<T>::insert(asset_id, id, ());

            if let Some(project_id) = project_id {
                ProjectIdByAssetId::<T>::insert(id, project_id.clone());
                AssetIdByProjectId::<T>::mutate_exists(project_id, |tokens| {
                    match tokens.as_mut() {
                        None => *tokens = Some(vec![id]),
                        Some(c) => c.push(id),
                    };
                });
            }

            Ok(post_dispatch_info)
        }

        fn deip_issue_asset_impl(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            beneficiary: T::AccountId,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResultWithPostInfo {
            let beneficiary_source = <T::Lookup as StaticLookup>::unlookup(beneficiary.clone());

            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call = pallet_assets::Call::<T>::mint {
                id: asset_id,
                beneficiary: beneficiary_source,
                amount,
            };
            let result = call.dispatch_bypass_filter(origin)?;

            if Self::try_get_tokenized_project(&id).is_some() {
                FtBalanceMap::<T>::mutate_exists(id, |maybe| {
                    let balances = match maybe.as_mut() {
                        None => {
                            *maybe = Some(vec![beneficiary]);
                            return
                        },
                        Some(b) => b,
                    };

                    let account = beneficiary;
                    match balances.binary_search_by_key(&&account, |a| a) {
                        Ok(_) => (),
                        Err(i) => balances.insert(i, account),
                    };
                });
            }

            Ok(result)
        }

        fn deip_set_metadata_impl(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            name: Vec<u8>,
            symbol: Vec<u8>,
            decimals: u8,
        ) -> DispatchResultWithPostInfo {
            let asset_name = name.clone();
            let asset_symbol = symbol.clone();

            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call =
                pallet_assets::Call::<T>::set_metadata { id: asset_id, name, symbol, decimals };
            let result = call.dispatch_bypass_filter(origin)?;

            AssetMetadataMap::<T>::insert(
                id,
                AssetMetadata { name: asset_name, symbol: asset_symbol, decimals },
            );

            Ok(result)
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(AssetsWeightInfoOf::<T>::create())]
        pub fn create(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            admin: <T::Lookup as StaticLookup>::Source,
            min_balance: AssetsBalanceOf<T>,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::create(origin, id, admin, min_balance)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::destroy(0, 0, 0))] // @TODO replace with actual coeff
        pub fn destroy(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            witness: DestroyWitness,
        ) -> DispatchResultWithPostInfo {
            pallet_assets::Pallet::<T>::destroy(origin, id, witness)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::mint())]
        pub fn mint(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            beneficiary: <T::Lookup as StaticLookup>::Source,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::mint(origin, id, beneficiary, amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::burn())]
        pub fn burn(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            who: <T::Lookup as StaticLookup>::Source,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::burn(origin, id, who, amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::transfer())]
        pub fn transfer(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            target: <T::Lookup as StaticLookup>::Source,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::transfer(origin, id, target, amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::transfer_keep_alive())]
        pub fn transfer_keep_alive(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            target: <T::Lookup as StaticLookup>::Source,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::transfer_keep_alive(origin, id, target, amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::freeze())]
        pub fn freeze(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            who: <T::Lookup as StaticLookup>::Source,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::freeze(origin, id, who)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::thaw())]
        pub fn thaw(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            who: <T::Lookup as StaticLookup>::Source,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::thaw(origin, id, who)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::freeze_asset())]
        pub fn freeze_asset(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::freeze_asset(origin, id)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::thaw_asset())]
        pub fn thaw_asset(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::thaw_asset(origin, id)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::transfer_ownership())]
        pub fn transfer_ownership(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            owner: <T::Lookup as StaticLookup>::Source,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::transfer_ownership(origin, id, owner)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::set_team())]
        pub fn set_team(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            issuer: <T::Lookup as StaticLookup>::Source,
            admin: <T::Lookup as StaticLookup>::Source,
            freezer: <T::Lookup as StaticLookup>::Source,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::set_team(origin, id, issuer, admin, freezer)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::set_metadata(name.len() as u32, symbol.len() as u32))]
        pub fn set_metadata(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            name: Vec<u8>,
            symbol: Vec<u8>,
            decimals: u8,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::set_metadata(origin, id, name, symbol, decimals)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::clear_metadata())]
        pub fn clear_metadata(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::clear_metadata(origin, id)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::approve_transfer())]
        pub fn approve_transfer(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            delegate: <T::Lookup as StaticLookup>::Source,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::approve_transfer(origin, id, delegate, amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::cancel_approval())]
        pub fn cancel_approval(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            delegate: <T::Lookup as StaticLookup>::Source,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::cancel_approval(origin, id, delegate)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::transfer_approved())]
        pub fn transfer_approved(
            origin: OriginFor<T>,
            id: <T as pallet_assets::Config>::AssetId,
            owner: <T::Lookup as StaticLookup>::Source,
            destination: <T::Lookup as StaticLookup>::Source,
            amount: AssetsBalanceOf<T>,
        ) -> DispatchResult {
            pallet_assets::Pallet::<T>::transfer_approved(origin, id, owner, destination, amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::create())]
        pub fn deip_create_asset(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            admin: T::DeipAccountId,
            min_balance: AssetsBalanceOf<T>,
            project_id: Option<DeipProjectIdOf<T>>,
        ) -> DispatchResultWithPostInfo {
            Self::deip_create_asset_impl(origin, id, admin.into(), min_balance, project_id)
        }

        #[pallet::weight((10_000, Pays::No))]
        pub fn deip_destroy(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            witness: pallet_assets::DestroyWitness,
        ) -> DispatchResultWithPostInfo {
            ensure!(
                !ProjectIdByAssetId::<T>::contains_key(id),
                Error::<T>::ProjectSecurityTokenCannotBeDestroyed
            );

            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;

            let call = pallet_assets::Call::<T>::destroy { id: asset_id, witness };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::mint())]
        pub fn deip_issue_asset(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            beneficiary: T::DeipAccountId,
            #[pallet::compact] amount: AssetsBalanceOf<T>,
        ) -> DispatchResultWithPostInfo {
            Self::deip_issue_asset_impl(origin, id, beneficiary.into(), amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::burn())]
        pub fn deip_burn(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            who: T::DeipAccountId,
            #[pallet::compact] amount: AssetsBalanceOf<T>,
        ) -> DispatchResultWithPostInfo {
            ensure!(
                !ProjectIdByAssetId::<T>::contains_key(id),
                Error::<T>::ProjectSecurityTokenCannotBeBurned
            );

            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;

            let who_source = <T::Lookup as StaticLookup>::unlookup(who.into());
            let call = pallet_assets::Call::<T>::burn { id: asset_id, who: who_source, amount };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::transfer())]
        pub fn deip_transfer(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            target: T::DeipAccountId,
            #[pallet::compact] amount: AssetsBalanceOf<T>,
        ) -> DispatchResultWithPostInfo {
            Self::deip_transfer_impl(origin, id, target.into(), amount)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::freeze())]
        pub fn deip_freeze(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            who: T::DeipAccountId,
        ) -> DispatchResultWithPostInfo {
            ensure!(
                !ProjectIdByAssetId::<T>::contains_key(id),
                Error::<T>::ProjectSecurityTokenAccountCannotBeFreezed
            );

            ensure!(
                !InvestmentByAssetId::<T>::contains_key(id),
                Error::<T>::ReservedAssetAccountCannotBeFreezed
            );

            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let who_source = <T::Lookup as StaticLookup>::unlookup(who.into());
            let call = pallet_assets::Call::<T>::freeze { id: asset_id, who: who_source };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::thaw())]
        pub fn deip_thaw(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            who: T::DeipAccountId,
        ) -> DispatchResultWithPostInfo {
            let who_source = <T::Lookup as StaticLookup>::unlookup(who.into());
            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call = pallet_assets::Call::<T>::thaw { id: asset_id, who: who_source };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::freeze_asset())]
        pub fn deip_freeze_asset(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
        ) -> DispatchResultWithPostInfo {
            ensure!(
                !ProjectIdByAssetId::<T>::contains_key(id),
                Error::<T>::ProjectSecurityTokenCannotBeFreezed
            );

            ensure!(
                !InvestmentByAssetId::<T>::contains_key(id),
                Error::<T>::ReservedAssetCannotBeFreezed
            );

            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call = pallet_assets::Call::<T>::freeze_asset { id: asset_id };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::thaw_asset())]
        pub fn deip_thaw_asset(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
        ) -> DispatchResultWithPostInfo {
            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call = pallet_assets::Call::<T>::thaw_asset { id: asset_id };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::transfer_ownership())]
        pub fn deip_transfer_ownership(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            owner: T::DeipAccountId,
        ) -> DispatchResultWithPostInfo {
            let owner_source = <T::Lookup as StaticLookup>::unlookup(owner.into());
            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call =
                pallet_assets::Call::<T>::transfer_ownership { id: asset_id, owner: owner_source };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::set_team())]
        pub fn deip_set_team(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            issuer: T::DeipAccountId,
            admin: T::DeipAccountId,
            freezer: T::DeipAccountId,
        ) -> DispatchResultWithPostInfo {
            let issuer_source = <T::Lookup as StaticLookup>::unlookup(issuer.into());
            let admin_source = <T::Lookup as StaticLookup>::unlookup(admin.into());
            let freezer_source = <T::Lookup as StaticLookup>::unlookup(freezer.into());
            let asset_id = AssetIdByDeipAssetId::<T>::iter_prefix(id)
                .next()
                .ok_or(Error::<T>::DeipAssetIdDoesNotExist)?
                .0;
            let call = pallet_assets::Call::<T>::set_team {
                id: asset_id,
                issuer: issuer_source,
                admin: admin_source,
                freezer: freezer_source,
            };
            call.dispatch_bypass_filter(origin)
        }

        #[pallet::weight(AssetsWeightInfoOf::<T>::set_metadata(name.len() as u32, symbol.len() as u32))]
        pub fn deip_set_metadata(
            origin: OriginFor<T>,
            id: DeipAssetIdOf<T>,
            name: Vec<u8>,
            symbol: Vec<u8>,
            decimals: u8,
        ) -> DispatchResultWithPostInfo {
            Self::deip_set_metadata_impl(origin, id, name, symbol, decimals)
        }

        #[pallet::weight(10_000)]
        pub fn deip_wipe_zero_balance(
            origin: OriginFor<T>,
            asset: DeipAssetIdOf<T>,
            account: AccountIdOf<T>,
        ) -> DispatchResultWithPostInfo {
            ensure_none(origin)?;

            FtBalanceMap::<T>::mutate_exists(asset, |maybe| match maybe.as_mut() {
                None => Err(Error::<T>::FtNotFound.into()),
                Some(b) => match b.binary_search_by_key(&&account, |a| a) {
                    Err(_) => Err(Error::<T>::FtBalanceNotFound.into()),
                    Ok(i) => {
                        b.remove(i);
                        if b.is_empty() {
                            *maybe = None;
                        }
                        Ok(Some(0).into())
                    },
                },
            })
        }
    }
}

#[cfg(feature = "std")]
impl<T: Config> GenesisConfig<T> {
    /// Direct implementation of `GenesisBuild::build_storage`.
    ///
    /// Kept in order not to break dependency.
    pub fn build_storage(&self) -> Result<sp_runtime::Storage, String> {
        <Self as frame_support::traits::GenesisBuild<T>>::build_storage(self)
    }

    /// Direct implementation of `GenesisBuild::assimilate_storage`.
    ///
    /// Kept in order not to break dependency.
    pub fn assimilate_storage(&self, storage: &mut sp_runtime::Storage) -> Result<(), String> {
        <Self as frame_support::traits::GenesisBuild<T>>::assimilate_storage(self, storage)
    }
}
