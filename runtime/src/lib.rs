// This file is part of CORD – https://cord.network

// Copyright (C) Dhiway Networks Pvt. Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later

// CORD is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// CORD is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with CORD. If not, see <https://www.gnu.org/licenses/>.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limits.
#![recursion_limit = "512"]

use codec::{Decode, Encode};
use scale_info::TypeInfo;

use cord_primitives::{
	prod_or_fast, AccountIndex, Balance, BlockNumber, DidIdentifier, Hash, Moment, Nonce,
};
pub use cord_primitives::{AccountId, Signature};
pub use identifier::Ss58Identifier;

use frame_support::{
	construct_runtime, derive_impl,
	dispatch::DispatchClass,
	genesis_builder_helper::{build_config, create_default_config},
	parameter_types,
	traits::{
		fungible::HoldConsideration, ConstU32, Contains, EitherOfDiverse, KeyOwnerProofSystem,
		LinearStoragePrice, PrivilegeCmp,
	},
	weights::{
		constants::{
			BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight, WEIGHT_REF_TIME_PER_MILLIS,
			WEIGHT_REF_TIME_PER_SECOND,
		},
		Weight,
	},
};
use frame_system::{
	limits::{BlockLength, BlockWeights},
	EnsureRoot,
};

#[cfg(feature = "runtime-benchmarks")]
use frame_system::EnsureSigned;

use sp_consensus_grandpa::AuthorityId as GrandpaId;

use pallet_identity::legacy::IdentityInfo;
use pallet_im_online::sr25519::AuthorityId as ImOnlineId;
use pallet_session::historical as pallet_session_historical;
use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_inherents::{CheckInherentsResult, InherentData};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{
		AccountIdLookup, BlakeTwo256, Block as BlockT, Extrinsic as ExtrinsicT, NumberFor,
		OpaqueKeys, SaturatedConversion, Verify,
	},
	transaction_validity::{TransactionPriority, TransactionSource, TransactionValidity},
	ApplyExtrinsicResult, Perbill,
};
use sp_staking::SessionIndex;
use sp_std::{cmp::Ordering, prelude::*};

#[cfg(any(feature = "std", test))]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;
use static_assertions::const_assert;

#[cfg(any(feature = "std", test))]
pub use frame_system::Call as SystemCall;
#[cfg(any(feature = "std", test))]
pub use pallet_balances::Call as BalancesCall;
#[cfg(any(feature = "std", test))]
pub use pallet_staking::StakerStatus;
#[cfg(any(feature = "std", test))]
pub use pallet_sudo::Call as SudoCall;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

/// Constant values used within the runtime.
use cord_runtime_constants::{currency::*, time::*};

// Weights used in the runtime.
mod weights;
// CORD Pallets
pub use authority_membership;
pub use pallet_network_membership;
pub mod benchmark;
pub use benchmark::DummySignature;
use pallet_network_membership::RuntimeDispatchWeightInfo;

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

/// Max size for serialized extrinsic params for this testing runtime.
/// This is a quite arbitrary but empirically battle tested value.
#[cfg(test)]
pub const CALL_PARAMS_MAX_SIZE: usize = 244;

/// Wasm binary unwrapped. If built with `SKIP_WASM_BUILD`, the function panics.
#[cfg(feature = "std")]
pub fn wasm_binary_unwrap() -> &'static [u8] {
	WASM_BINARY.expect(
		"Development wasm binary is not available. This means the client is \
  		 built with `SKIP_WASM_BUILD` flag and it is only usable for \
		 production chains. Please rebuild with the flag disabled.",
	)
}

/// Runtime version.
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("cord"),
	impl_name: create_runtime_str!("dhiway-cord"),
	authoring_version: 0,
	spec_version: 9200,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 2,
	state_version: 1,
};

/// The BABE epoch configuration at genesis.
pub const BABE_GENESIS_EPOCH_CONFIG: sp_consensus_babe::BabeEpochConfiguration =
	sp_consensus_babe::BabeEpochConfiguration {
		c: PRIMARY_PROBABILITY,
		allowed_slots: sp_consensus_babe::AllowedSlots::PrimaryAndSecondaryVRFSlots,
	};

/// Native version.
#[cfg(any(feature = "std", test))]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

/// We currently allow all calls.
pub struct BaseFilter;
impl Contains<RuntimeCall> for BaseFilter {
	fn contains(_c: &RuntimeCall) -> bool {
		true
	}
}

type MoreThanHalfCouncil = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionMoreThan<AccountId, CouncilCollective, 1, 2>,
>;

type EnsureRootOrCommitteeApproval = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionMoreThan<AccountId, TechnicalCollective, 3, 5>,
>;

/// We assume that an on-initialize consumes 10% of the weight on average, hence
/// a single extrinsic will not be allowed to consume more than
/// `AvailableBlockRatio - 10%`.
pub const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(10);
/// We allow `Normal` extrinsics to fill up the block up to 50%, the rest can be
/// used by  Operational  extrinsics.
pub const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(50);
// We'll verify that WEIGHT_REF_TIME_PER_SECOND does not overflow, allowing us to use
// simple multiply and divide operators instead of saturating or checked ones.
const_assert!(WEIGHT_REF_TIME_PER_SECOND.checked_div(3).is_some());
const_assert!((WEIGHT_REF_TIME_PER_SECOND / 3).checked_mul(2).is_some());
/// We allow for 1/3 of block time for computations, with maximum proof size.
/// It's 3/3 sec for cord runtime with 3 second block duration.
const MAXIMUM_BLOCK_WEIGHT: Weight =
	Weight::from_parts(WEIGHT_REF_TIME_PER_MILLIS * MILLISECS_PER_BLOCK / 3, u64::MAX);

const_assert!(NORMAL_DISPATCH_RATIO.deconstruct() >= AVERAGE_ON_INITIALIZE_RATIO.deconstruct());

pub fn block_weights_for(maximum_block_weight: Weight) -> BlockWeights {
	BlockWeights::builder()
		.base_block(BlockExecutionWeight::get())
		.for_class(DispatchClass::all(), |weights| {
			weights.base_extrinsic = ExtrinsicBaseWeight::get();
		})
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * maximum_block_weight);
		})
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(maximum_block_weight);
			// Operational transactions have some extra reserved space, so that they
			// are included even if block reached `MAXIMUM_BLOCK_WEIGHT`.
			weights.reserved =
				Some(maximum_block_weight - NORMAL_DISPATCH_RATIO * maximum_block_weight);
		})
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic()
}

parameter_types! {
   pub const BlockHashCount: BlockNumber = 2400;
   pub const Version: RuntimeVersion = VERSION;
   pub RuntimeBlockWeights: BlockWeights = block_weights_for(MAXIMUM_BLOCK_WEIGHT);
   pub RuntimeBlockLength: BlockLength =
	   BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
   pub const SS58Prefix: u8 = 29;
}

#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig as frame_system::DefaultConfig)]
impl frame_system::Config for Runtime {
	type BaseCallFilter = BaseFilter;
	type BlockWeights = RuntimeBlockWeights;
	type BlockLength = RuntimeBlockLength;
	type DbWeight = RocksDbWeight;
	type Nonce = Nonce;
	type Hash = Hash;
	type AccountId = AccountId;
	type Lookup = AccountIdLookup<AccountId, ()>;
	type Block = Block;
	type BlockHashCount = BlockHashCount;
	type Version = Version;
	type AccountData = pallet_balances::AccountData<Balance>;
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
	type SS58Prefix = SS58Prefix;
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub MaximumSchedulerWeight: Weight = Perbill::from_percent(80) * RuntimeBlockWeights::get().max_block;
	pub const MaxScheduledPerBlock: u32 = 50;
	pub const NoPreimagePostponement: Option<u32> = Some(10);
}

/// Used the compare the privilege of an origin inside the scheduler.
pub struct OriginPrivilegeCmp;

impl PrivilegeCmp<OriginCaller> for OriginPrivilegeCmp {
	fn cmp_privilege(left: &OriginCaller, right: &OriginCaller) -> Option<Ordering> {
		if left == right {
			return Some(Ordering::Equal);
		}

		match (left, right) {
			// Root is greater than anything.
			(OriginCaller::system(frame_system::RawOrigin::Root), _) => Some(Ordering::Greater),
			// Check which one has more yes votes.
			(
				OriginCaller::Council(pallet_collective::RawOrigin::Members(l_yes_votes, l_count)),
				OriginCaller::Council(pallet_collective::RawOrigin::Members(r_yes_votes, r_count)),
			) => Some((l_yes_votes * r_count).cmp(&(r_yes_votes * l_count))),
			// For every other origin we don't care, as they are not used for `ScheduleOrigin`.
			_ => None,
		}
	}
}

impl pallet_scheduler::Config for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeEvent = RuntimeEvent;
	type PalletsOrigin = OriginCaller;
	type RuntimeCall = RuntimeCall;
	type MaximumWeight = MaximumSchedulerWeight;
	type ScheduleOrigin = EnsureRootOrCommitteeApproval;
	type MaxScheduledPerBlock = MaxScheduledPerBlock;
	type WeightInfo = weights::pallet_scheduler::WeightInfo<Runtime>;
	type OriginPrivilegeCmp = OriginPrivilegeCmp;
	type Preimages = Preimage;
}

parameter_types! {
	pub const PreimageMaxSize: u32 = 4096 * 1024;
	pub const PreimageBaseDeposit: Balance = deposit(2, 64);
	pub const PreimageByteDeposit: Balance = deposit(0, 1);
	pub const PreimageHoldReason: RuntimeHoldReason = RuntimeHoldReason::Preimage(pallet_preimage::HoldReason::Preimage);

}

impl pallet_preimage::Config for Runtime {
	type WeightInfo = weights::pallet_preimage::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type ManagerOrigin = EnsureRoot<AccountId>;
	type Consideration = HoldConsideration<
		AccountId,
		Balances,
		PreimageHoldReason,
		LinearStoragePrice<PreimageBaseDeposit, PreimageByteDeposit, Balance>,
	>;
}

parameter_types! {
	pub EpochDuration: u64 = EPOCH_DURATION_IN_SLOTS;
	pub const ExpectedBlockTime: Moment = MILLISECS_PER_BLOCK;
	pub ReportLongevity: u64 =
		BondingDuration::get() as u64 * SessionsPerEra::get() as u64 * EpochDuration::get();
	pub const MaxAuthorities: u32 = 1_000;
}

impl pallet_babe::Config for Runtime {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;
	// session module is the trigger
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;
	type DisabledValidators = Session;
	type WeightInfo = ();
	type MaxAuthorities = MaxAuthorities;
	type MaxNominators = ConstU32<0>;
	type KeyOwnerProof =
		<Historical as KeyOwnerProofSystem<(KeyTypeId, pallet_babe::AuthorityId)>>::Proof;
	type EquivocationReportSystem =
		pallet_babe::EquivocationReportSystem<Self, Offences, Historical, ReportLongevity>;
}

parameter_types! {
	pub const IndexDeposit: Balance =  EXISTENTIAL_DEPOSIT;
}

impl pallet_indices::Config for Runtime {
	type AccountIndex = AccountIndex;
	type Currency = Balances;
	type Deposit = IndexDeposit;
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_indices::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = EXISTENTIAL_DEPOSIT;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type MaxLocks = MaxLocks;
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type Balance = Balance;
	type DustRemoval = ();
	type RuntimeEvent = RuntimeEvent;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type FreezeIdentifier = RuntimeFreezeReason;
	type MaxFreezes = ConstU32<1>;
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
}

parameter_types! {
		pub MinimumPeriod: u64 = prod_or_fast!(
		MINIMUM_DURATION,
		500_u64,
		"CORD_MINIMUM_DURATION"
	);
}

impl pallet_timestamp::Config for Runtime {
	type Moment = u64;
	type OnTimestampSet = Babe;
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = weights::pallet_timestamp::WeightInfo<Runtime>;
}

impl pallet_authorship::Config for Runtime {
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Babe>;
	type EventHandler = ImOnline;
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub grandpa: Grandpa,
		pub babe: Babe,
		pub im_online: ImOnline,
		pub authority_discovery: AuthorityDiscovery,
	}
}

/// Special `ValidatorIdOf` implementation that is just returning the input as
/// result.
pub struct ValidatorIdOf;
impl sp_runtime::traits::Convert<AccountId, Option<AccountId>> for ValidatorIdOf {
	fn convert(a: AccountId) -> Option<AccountId> {
		Some(a)
	}
}

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = AccountId;
	type ValidatorIdOf = ValidatorIdOf;
	type ShouldEndSession = Babe;
	type NextSessionRotation = Babe;
	type SessionManager = pallet_session::historical::NoteHistoricalRoot<Self, AuthorityMembership>;
	type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
}

pub struct FullIdentificationOf;
impl sp_runtime::traits::Convert<AccountId, Option<()>> for FullIdentificationOf {
	fn convert(_: AccountId) -> Option<()> {
		Some(Default::default())
	}
}

impl pallet_session::historical::Config for Runtime {
	type FullIdentification = ();
	type FullIdentificationOf = FullIdentificationOf;
}

parameter_types! {
	pub const SessionsPerEra: SessionIndex = 6;
	pub const BondingDuration: sp_staking::EraIndex = 28;
}

parameter_types! {
	pub const MaxSubAccounts: u32 = 100;
	pub const MaxRegistrars: u32 = 20;
	pub const MaxAdditionalFields: u32 = 20;
}

impl pallet_identity::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type MaxSubAccounts = MaxSubAccounts;
	type IdentityInformation = IdentityInfo<MaxAdditionalFields>;
	type MaxRegistrars = MaxRegistrars;
	type RegistrarOrigin = MoreThanHalfCouncil;
	type OffchainSignature = Signature;
	type SigningPublicKey = <Signature as Verify>::Signer;
	type UsernameAuthorityOrigin = MoreThanHalfCouncil;
	type PendingUsernameExpiration = ConstU32<{ 7 * DAYS }>;
	type MaxSuffixLength = ConstU32<7>;
	type MaxUsernameLength = ConstU32<32>;
	type WeightInfo = pallet_identity::weights::SubstrateWeight<Runtime>;
}

// parameter_types! {
// 	// Minimum 4 CENTS/byte
// 	pub const MaxAdditionalFields: u32 = 10;
// 	pub const MaxRegistrars: u32 = 25;
// }

// impl pallet_identity::Config for Runtime {
// 	type RuntimeEvent = RuntimeEvent;
// 	type MaxAdditionalFields = MaxAdditionalFields;
// 	type IdentityInformation = IdentityInfo<MaxAdditionalFields>;
// 	type MaxRegistrars = MaxRegistrars;
// 	type RegistrarOrigin = MoreThanHalfCouncil;
// 	type WeightInfo = weights::pallet_identity::WeightInfo<Runtime>;
// }

parameter_types! {
	pub MotionDuration: BlockNumber = prod_or_fast!(3 * DAYS, 2 * MINUTES, "CORD_MOTION_DURATION");
	pub const MaxProposals: u32 = 100;
	pub const MaxMembers: u32 = 50;
	pub MaxProposalWeight: Weight = Perbill::from_percent(80) * RuntimeBlockWeights::get().max_block;
}

type CouncilCollective = pallet_collective::Instance1;
impl pallet_collective::Config<CouncilCollective> for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type Proposal = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type MotionDuration = MotionDuration;
	type MaxProposals = MaxProposals;
	type MaxMembers = MaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type WeightInfo = weights::pallet_collective::WeightInfo<Runtime>;
	type NetworkMembershipOrigin = EnsureRoot<Self::AccountId>;
	type MaxProposalWeight = MaxProposalWeight;
}

impl pallet_membership::Config<pallet_membership::Instance1> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type AddOrigin = MoreThanHalfCouncil;
	type RemoveOrigin = MoreThanHalfCouncil;
	type SwapOrigin = MoreThanHalfCouncil;
	type ResetOrigin = MoreThanHalfCouncil;
	type PrimeOrigin = MoreThanHalfCouncil;
	type MembershipInitialized = Council;
	type MembershipChanged = Council;
	type MaxMembers = MaxMembers;
	type WeightInfo = weights::pallet_membership::WeightInfo<Runtime>;
}

type TechnicalCollective = pallet_collective::Instance2;
impl pallet_collective::Config<TechnicalCollective> for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type Proposal = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type MotionDuration = MotionDuration;
	type MaxProposals = MaxProposals;
	type MaxMembers = MaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type WeightInfo = weights::pallet_collective::WeightInfo<Runtime>;
	type NetworkMembershipOrigin = EnsureRoot<Self::AccountId>;
	type MaxProposalWeight = MaxProposalWeight;
}

impl pallet_membership::Config<pallet_membership::Instance2> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type AddOrigin = MoreThanHalfCouncil;
	type RemoveOrigin = MoreThanHalfCouncil;
	type SwapOrigin = MoreThanHalfCouncil;
	type ResetOrigin = MoreThanHalfCouncil;
	type PrimeOrigin = MoreThanHalfCouncil;
	type MembershipInitialized = TechnicalCommittee;
	type MembershipChanged = TechnicalCommittee;
	type MaxMembers = MaxMembers;
	type WeightInfo = weights::pallet_membership::WeightInfo<Runtime>;
}

impl pallet_offences::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type IdentificationTuple = pallet_session::historical::IdentificationTuple<Self>;
	type OnOffenceHandler = AuthorityMembership;
}

impl pallet_authority_discovery::Config for Runtime {
	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub const ImOnlineUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
	pub const MaxPeerInHeartbeats: u32 = 10_000;
	pub const MaxPeerDataEncodingSize: u32 = 1_000;
	pub const MaxKeys: u32 = 100_000;
}

impl pallet_im_online::Config for Runtime {
	type AuthorityId = ImOnlineId;
	type RuntimeEvent = RuntimeEvent;
	type ValidatorSet = Historical;
	type NextSessionRotation = Babe;
	type ReportUnresponsiveness = Offences;
	type UnsignedPriority = ImOnlineUnsignedPriority;
	type WeightInfo = weights::pallet_im_online::WeightInfo<Runtime>;
	type MaxKeys = MaxKeys;
	type MaxPeerInHeartbeats = MaxPeerInHeartbeats;
}

parameter_types! {
	pub const MaxSetIdSessionEntries: u32 = BondingDuration::get() * SessionsPerEra::get();
}

impl pallet_grandpa::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;

	type WeightInfo = ();
	type MaxAuthorities = MaxAuthorities;
	type MaxNominators = ();
	type MaxSetIdSessionEntries = MaxSetIdSessionEntries;
	type KeyOwnerProof = <Historical as KeyOwnerProofSystem<(KeyTypeId, GrandpaId)>>::Proof;
	type EquivocationReportSystem =
		pallet_grandpa::EquivocationReportSystem<Self, Offences, Historical, ReportLongevity>;
}

/// Submits a transaction with the node's public and signature type. Adheres to
/// the signed extension format of the chain.
impl<LocalCall> frame_system::offchain::CreateSignedTransaction<LocalCall> for Runtime
where
	RuntimeCall: From<LocalCall>,
{
	fn create_transaction<C: frame_system::offchain::AppCrypto<Self::Public, Self::Signature>>(
		call: RuntimeCall,
		public: <Signature as Verify>::Signer,
		account: AccountId,
		nonce: Nonce,
	) -> Option<(RuntimeCall, <UncheckedExtrinsic as ExtrinsicT>::SignaturePayload)> {
		use sp_runtime::traits::StaticLookup;
		// take the biggest period possible.
		let period =
			BlockHashCount::get().checked_next_power_of_two().map(|c| c / 2).unwrap_or(2) as u64;

		let current_block = System::block_number()
			.saturated_into::<u64>()
			// The `System::block_number` is initialized with `n+1`,
			// so the actual block number is `n`.
			.saturating_sub(1);
		let extra: SignedExtra = (
			pallet_network_membership::CheckNetworkMembership::<Runtime>::new(),
			frame_system::CheckNonZeroSender::<Runtime>::new(),
			frame_system::CheckSpecVersion::<Runtime>::new(),
			frame_system::CheckTxVersion::<Runtime>::new(),
			frame_system::CheckGenesis::<Runtime>::new(),
			frame_system::CheckMortality::<Runtime>::from(generic::Era::mortal(
				period,
				current_block,
			)),
			frame_system::CheckNonce::<Runtime>::from(nonce),
			frame_system::CheckWeight::<Runtime>::new(),
		);
		let raw_payload = SignedPayload::new(call, extra)
			.map_err(|e| {
				log::warn!("Unable to create signed payload: {:?}", e);
			})
			.ok()?;
		let signature = raw_payload.using_encoded(|payload| C::sign(payload, public))?;
		let (call, extra, _) = raw_payload.deconstruct();
		let address = <Runtime as frame_system::Config>::Lookup::unlookup(account);
		Some((call, (address, signature, extra)))
	}
}

impl frame_system::offchain::SigningTypes for Runtime {
	type Public = <Signature as Verify>::Signer;
	type Signature = Signature;
}

impl<C> frame_system::offchain::SendTransactionTypes<C> for Runtime
where
	RuntimeCall: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = RuntimeCall;
}

impl pallet_utility::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type PalletsOrigin = OriginCaller;
	type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

parameter_types! {
	// One storage item; key size is 32; value is size 4+4+16+32 bytes = 56 bytes.
	pub const DepositBase: Balance = deposit(1, 88);
	// Additional storage item size of 32 bytes.
	pub const DepositFactor: Balance = deposit(0, 32);
	pub const MaxSignatories: u16 = 100;
}

impl pallet_multisig::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type DepositBase = DepositBase;
	type DepositFactor = DepositFactor;
	type MaxSignatories = MaxSignatories;
	type WeightInfo = weights::pallet_multisig::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MaxProposalLength: u16 = 5;
}
impl authority_membership::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type AuthorityMembershipOrigin = MoreThanHalfCouncil;
}

parameter_types! {
	pub const MaxWellKnownNodes: u32 = 1_000;
	pub const MaxPeerIdLength: u32 = 128;
	pub const MaxNodeIdLength: u32 = 53;
}

impl pallet_node_authorization::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type MaxWellKnownNodes = MaxWellKnownNodes;
	type MaxPeerIdLength = MaxPeerIdLength;
	type MaxNodeIdLength = MaxNodeIdLength;
	type NodeAuthorizationOrigin = MoreThanHalfCouncil;
	type WeightInfo = ();
}

parameter_types! {
	pub const MembershipPeriod: BlockNumber = YEAR;
	pub const MaxMembersPerBlock: u32 = 1_000;
	pub const MaxEventsHistory: u32 = u32::MAX;
}

impl pallet_network_membership::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type NetworkMembershipOrigin = MoreThanHalfCouncil;
	type MembershipPeriod = MembershipPeriod;
	type MaxMembersPerBlock = MaxMembersPerBlock;
	type WeightInfo = weights::pallet_network_membership::WeightInfo<Runtime>;
}

impl identifier::Config for Runtime {
	type MaxEventsHistory = MaxEventsHistory;
}

impl pallet_runtime_upgrade::Config for Runtime {
	type SetCodeOrigin = EnsureRootOrCommitteeApproval;
}

parameter_types! {
	#[derive(Debug, Clone, Eq, PartialEq, TypeInfo, Decode, Encode)]
	pub const MaxNewKeyAgreementKeys: u32 = 10;
	#[derive(Clone)]
	pub const MaxPublicKeysPerDid: u32 = 20;
	#[derive(Debug, Clone, Eq, PartialEq)]
	pub const MaxTotalKeyAgreementKeys: u32 = 19;
	pub const MaxBlocksTxValidity: BlockNumber =  2 * HOURS;
	pub const MaxNumberOfServicesPerDid: u32 = 25;
	pub const MaxServiceIdLength: u32 = 50;
	pub const MaxServiceTypeLength: u32 = 50;
	pub const MaxServiceUrlLength: u32 = 200;
	pub const MaxNumberOfTypesPerService: u32 = 1;
	pub const MaxNumberOfUrlsPerService: u32 = 1;
}

impl pallet_did::Config for Runtime {
	type DidIdentifier = DidIdentifier;
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type RuntimeOrigin = RuntimeOrigin;

	#[cfg(not(feature = "runtime-benchmarks"))]
	type EnsureOrigin = pallet_did::EnsureDidOrigin<Self::DidIdentifier, AccountId>;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type OriginSuccess = pallet_did::DidRawOrigin<AccountId, Self::DidIdentifier>;
	#[cfg(feature = "runtime-benchmarks")]
	type EnsureOrigin = EnsureSigned<Self::DidIdentifier>;
	#[cfg(feature = "runtime-benchmarks")]
	type OriginSuccess = Self::DidIdentifier;

	type MaxNewKeyAgreementKeys = MaxNewKeyAgreementKeys;
	type MaxPublicKeysPerDid = MaxPublicKeysPerDid;
	type MaxTotalKeyAgreementKeys = MaxTotalKeyAgreementKeys;
	type MaxBlocksTxValidity = MaxBlocksTxValidity;
	type MaxNumberOfServicesPerDid = MaxNumberOfServicesPerDid;
	type MaxServiceIdLength = MaxServiceIdLength;
	type MaxServiceTypeLength = MaxServiceTypeLength;
	type MaxServiceUrlLength = MaxServiceUrlLength;
	type MaxNumberOfTypesPerService = MaxNumberOfTypesPerService;
	type MaxNumberOfUrlsPerService = MaxNumberOfUrlsPerService;
	type WeightInfo = weights::pallet_did::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MinNameLength: u32 = 3;
	pub const MaxNameLength: u32 = 64;
	pub const MaxPrefixLength: u32 = 54;
}

impl pallet_did_name::Config for Runtime {
	type BanOrigin = EnsureRoot<AccountId>;
	type EnsureOrigin = pallet_did::EnsureDidOrigin<DidIdentifier, AccountId>;
	type OriginSuccess = pallet_did::DidRawOrigin<AccountId, DidIdentifier>;
	type RuntimeEvent = RuntimeEvent;
	type MaxNameLength = MaxNameLength;
	type MinNameLength = MinNameLength;
	type MaxPrefixLength = MaxPrefixLength;
	type DidName = pallet_did_name::did_name::AsciiDidName<Runtime>;
	type DidNameOwner = DidIdentifier;
	type WeightInfo = weights::pallet_did_name::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MaxEncodedSchemaLength: u32 = 15_360;
}

impl pallet_schema::Config for Runtime {
	type SchemaCreatorId = DidIdentifier;
	type EnsureOrigin = pallet_did::EnsureDidOrigin<DidIdentifier, AccountId>;
	type OriginSuccess = pallet_did::DidRawOrigin<AccountId, DidIdentifier>;
	type RuntimeEvent = RuntimeEvent;
	type MaxEncodedSchemaLength = MaxEncodedSchemaLength;
	type WeightInfo = weights::pallet_schema::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MaxSpaceDelegates: u32 = 10_000;
}

impl pallet_chain_space::Config for Runtime {
	type SpaceCreatorId = DidIdentifier;
	type EnsureOrigin = pallet_did::EnsureDidOrigin<DidIdentifier, AccountId>;
	type OriginSuccess = pallet_did::DidRawOrigin<AccountId, DidIdentifier>;
	type RuntimeEvent = RuntimeEvent;
	type ChainSpaceOrigin = MoreThanHalfCouncil;
	type MaxSpaceDelegates = MaxSpaceDelegates;
	type WeightInfo = weights::pallet_chain_space::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MaxDigestsPerBatch: u16 = 1_000;
	pub const MaxRemoveEntries: u16 = 1_000;
}

impl pallet_statement::Config for Runtime {
	type EnsureOrigin = pallet_did::EnsureDidOrigin<DidIdentifier, AccountId>;
	type OriginSuccess = pallet_did::DidRawOrigin<AccountId, DidIdentifier>;
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_statement::WeightInfo<Runtime>;
	type MaxDigestsPerBatch = MaxDigestsPerBatch;
	type MaxRemoveEntries = MaxRemoveEntries;
}

impl pallet_remark::Config for Runtime {
	type WeightInfo = weights::pallet_remark::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
}

impl pallet_sudo::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type WeightInfo = weights::pallet_sudo::WeightInfo<Runtime>;
}

impl pallet_network_score::Config for Runtime {
	type RatingProviderIdOf = DidIdentifier;
	type EnsureOrigin = pallet_did::EnsureDidOrigin<DidIdentifier, AccountId>;
	type OriginSuccess = pallet_did::DidRawOrigin<AccountId, DidIdentifier>;
	type RuntimeEvent = RuntimeEvent;
	type MaxEncodedValueLength = ConstU32<128>;
	type MaxRatingValue = ConstU32<50>;
	type WeightInfo = weights::pallet_network_score::WeightInfo<Runtime>;
}

parameter_types! {
	pub const MaxEncodedValueLength: u32 = 1_024;
	pub const MaxAssetDistribution: u32 = u32::MAX;
}

impl pallet_asset::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type EnsureOrigin = pallet_did::EnsureDidOrigin<DidIdentifier, AccountId>;
	type OriginSuccess = pallet_did::DidRawOrigin<AccountId, DidIdentifier>;
	type MaxEncodedValueLength = MaxEncodedValueLength;
	type MaxAssetDistribution = MaxAssetDistribution;
	type WeightInfo = weights::pallet_asset::WeightInfo<Runtime>;
}

construct_runtime! (
	pub enum Runtime
	{
		System: frame_system = 0,
		Scheduler: pallet_scheduler = 1,
		Babe: pallet_babe = 2,
		Timestamp: pallet_timestamp = 3,
		Indices: pallet_indices = 4,
		Balances: pallet_balances = 5,
		Authorship: pallet_authorship = 6,
		AuthorityMembership: authority_membership = 7,
		Offences: pallet_offences = 8,
		Session: pallet_session = 9,
		Grandpa: pallet_grandpa = 10,
		ImOnline: pallet_im_online = 11,
		AuthorityDiscovery: pallet_authority_discovery = 12,
		Preimage: pallet_preimage = 13,
		Council: pallet_collective::<Instance1> = 14,
		CouncilMembership: pallet_membership::<Instance1> = 15,
		TechnicalCommittee: pallet_collective::<Instance2> = 16,
		TechnicalMembership: pallet_membership::<Instance2> = 17,
		NodeAuthorization: pallet_node_authorization = 18,
		RuntimeUpgrade: pallet_runtime_upgrade = 19,
		Utility: pallet_utility = 31,
		Historical: pallet_session_historical = 33,
		Multisig: pallet_multisig = 35,
		Remark: pallet_remark = 37,
		Identity: pallet_identity = 38,
		Identifier: identifier = 39,
		NetworkMembership: pallet_network_membership =101,
		Did: pallet_did = 102,
		Schema: pallet_schema = 103,
		ChainSpace: pallet_chain_space = 104,
		Statement: pallet_statement = 105,
		DidName: pallet_did_name = 106,
		NetworkScore: pallet_network_score = 108,
		Asset: pallet_asset = 109,
		Sudo: pallet_sudo = 255,
	}
);

#[rustfmt::skip]
impl pallet_did::DeriveDidCallAuthorizationVerificationKeyRelationship for RuntimeCall {
	fn derive_verification_key_relationship(
		&self,
	) -> pallet_did::DeriveDidCallKeyRelationshipResult {
		fn single_key_relationship(
			calls: &[RuntimeCall],
		) -> pallet_did::DeriveDidCallKeyRelationshipResult {
			let init = calls
				.get(0)
				.ok_or(pallet_did::RelationshipDeriveError::InvalidCallParameter)?
				.derive_verification_key_relationship()?;
			calls
				.iter()
				.skip(1)
				.map(RuntimeCall::derive_verification_key_relationship)
				.try_fold(init, |acc, next| {
					if Ok(acc) == next {
						Ok(acc)
					} else {
						Err(pallet_did::RelationshipDeriveError::InvalidCallParameter)
					}
				})
		}
		match self {
			// DID creation is not allowed through the DID proxy.
			RuntimeCall::Did(pallet_did::Call::create { .. }) => {
				Err(pallet_did::RelationshipDeriveError::NotCallableByDid)
			},
			RuntimeCall::Did { .. } => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::DidName { .. } => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::Schema { .. } => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::Statement { .. } => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::NetworkScore { .. } => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::Asset { .. } => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::add_delegate { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::CapabilityDelegation)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::add_admin_delegate { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::CapabilityDelegation)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::add_delegator { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::CapabilityDelegation)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::remove_delegate { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::CapabilityDelegation)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::create { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::archive { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::restore { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::subspace_create { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::update_transaction_capacity { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::ChainSpace(pallet_chain_space::Call::update_transaction_capacity_sub { .. }) => {
				Ok(pallet_did::DidVerificationKeyRelationship::Authentication)
			},
			RuntimeCall::Utility(pallet_utility::Call::batch { calls }) => {
				single_key_relationship(&calls[..])
			},
			RuntimeCall::Utility(pallet_utility::Call::batch_all { calls }) => {
				single_key_relationship(&calls[..])
			},
			RuntimeCall::Utility(pallet_utility::Call::force_batch { calls }) => {
				single_key_relationship(&calls[..])
			},
			#[cfg(not(feature = "runtime-benchmarks"))]
			_ => Err(pallet_did::RelationshipDeriveError::NotCallableByDid),
			// By default, returns the authentication key
			#[cfg(feature = "runtime-benchmarks")]
			_ => Ok(pallet_did::DidVerificationKeyRelationship::Authentication),
		}
	}

	// Always return a System::remark() extrinsic call
	#[cfg(feature = "runtime-benchmarks")]
	fn get_call_for_did_call_benchmark() -> Self {
		RuntimeCall::System(frame_system::Call::remark { remark: vec![] })
	}
}

/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// `BlockId` type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// The `SignedExtension` to the basic transaction logic.
pub type SignedExtra = (
	pallet_network_membership::CheckNetworkMembership<Runtime>,
	frame_system::CheckNonZeroSender<Runtime>,
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckMortality<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
);

/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, RuntimeCall, SignedExtra>;

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
>;
/// The payload being signed in transactions.
pub type SignedPayload = generic::SignedPayload<RuntimeCall, SignedExtra>;

#[cfg(feature = "runtime-benchmarks")]
mod benches {
	frame_benchmarking::define_benchmarks!(
		[frame_benchmarking, BaselineBench::<Runtime>]
		[pallet_babe, Babe]
		[pallet_balances, Balances]
		[pallet_collective, Council]
		[pallet_grandpa, Grandpa]
		[pallet_identity, Identity]
		[pallet_im_online, ImOnline]
		[pallet_indices, Indices]
		[pallet_membership, TechnicalMembership]
		[pallet_multisig, Multisig]
		[pallet_preimage, Preimage]
		[pallet_remark, Remark]
		[pallet_scheduler, Scheduler]
		[frame_system, SystemBench::<Runtime>]
		[pallet_timestamp, Timestamp]
		[pallet_utility, Utility]
		[pallet_schema, Schema]
		[pallet_statement, Statement]
		[pallet_chain_space, ChainSpace]
		[pallet_did, Did]
		[pallet_did_name, DidName]
		[pallet_network_membership, NetworkMembership]
		[pallet_network_score, NetworkScore]
		[pallet_sudo, Sudo]
		[pallet_asset, Asset]
	);
}

sp_api::impl_runtime_apis! {
	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block);
		}

		fn initialize_block(header: &<Block as BlockT>::Header) {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}

		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> sp_std::vec::Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(block: Block, data: InherentData) -> CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_consensus_grandpa::GrandpaApi<Block> for Runtime {
		fn grandpa_authorities() -> sp_consensus_grandpa::AuthorityList {
			Grandpa::grandpa_authorities()
		}

		fn current_set_id() -> sp_consensus_grandpa::SetId {
			Grandpa::current_set_id()
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			equivocation_proof: sp_consensus_grandpa::EquivocationProof<
				<Block as BlockT>::Hash,
				NumberFor<Block>,
			>,
			key_owner_proof: sp_consensus_grandpa::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			let key_owner_proof = key_owner_proof.decode()?;

			Grandpa::submit_unsigned_equivocation_report(
				equivocation_proof,
				key_owner_proof,
			)
		}

		fn generate_key_ownership_proof(
			_set_id: sp_consensus_grandpa::SetId,
			authority_id: GrandpaId,
		) -> Option<sp_consensus_grandpa::OpaqueKeyOwnershipProof> {
			use codec::Encode;

			Historical::prove((sp_consensus_grandpa::KEY_TYPE, authority_id))
				.map(|p| p.encode())
				.map(sp_consensus_grandpa::OpaqueKeyOwnershipProof::new)
		}
	}

	impl sp_consensus_babe::BabeApi<Block> for Runtime {
		fn configuration() -> sp_consensus_babe::BabeConfiguration {
			let epoch_config = Babe::epoch_config().unwrap_or(BABE_GENESIS_EPOCH_CONFIG);
			sp_consensus_babe::BabeConfiguration {
				slot_duration: Babe::slot_duration(),
				epoch_length: EpochDuration::get(),
				c: epoch_config.c,
				authorities: Babe::authorities().to_vec(),
				randomness: Babe::randomness(),
				allowed_slots: epoch_config.allowed_slots,
			}
		}

		fn current_epoch_start() -> sp_consensus_babe::Slot {
			Babe::current_epoch_start()
		}

		fn current_epoch() -> sp_consensus_babe::Epoch {
			Babe::current_epoch()
		}

		fn next_epoch() -> sp_consensus_babe::Epoch {
			Babe::next_epoch()
		}

		fn generate_key_ownership_proof(
			_slot: sp_consensus_babe::Slot,
			authority_id: sp_consensus_babe::AuthorityId,
		) -> Option<sp_consensus_babe::OpaqueKeyOwnershipProof> {
			use codec::Encode;

			Historical::prove((sp_consensus_babe::KEY_TYPE, authority_id))
				.map(|p| p.encode())
				.map(sp_consensus_babe::OpaqueKeyOwnershipProof::new)
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			equivocation_proof: sp_consensus_babe::EquivocationProof<<Block as BlockT>::Header>,
			key_owner_proof: sp_consensus_babe::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			let key_owner_proof = key_owner_proof.decode()?;

			Babe::submit_unsigned_equivocation_report(
				equivocation_proof,
				key_owner_proof,
			)
		}
	}

	impl sp_authority_discovery::AuthorityDiscoveryApi<Block> for Runtime {
		fn authorities() -> Vec<AuthorityDiscoveryId> {
			AuthorityDiscovery::authorities()
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl pallet_did_runtime_api::DidApi<
		Block,
		DidIdentifier,
		AccountId,
		Hash,
		BlockNumber
	> for Runtime {
		fn query(did: DidIdentifier) -> Option<
			pallet_did_runtime_api::RawDidLinkedInfo<
				DidIdentifier,
				AccountId,
				Hash,
				BlockNumber
			>
		> {
			let details = pallet_did::Did::<Runtime>::get(&did)?;
			let name = pallet_did_name::Names::<Runtime>::get(&did).map(Into::into);
			let service_endpoints = pallet_did::ServiceEndpoints::<Runtime>::iter_prefix(&did).map(|e| From::from(e.1)).collect();

			Some(pallet_did_runtime_api::RawDidLinkedInfo {
				identifier: did.clone(),
				account: did,
				name,
				service_endpoints,
				details: details.into(),
			})
		}
		fn query_by_name(name: Vec<u8>) -> Option<pallet_did_runtime_api::RawDidLinkedInfo<
				DidIdentifier,
				AccountId,
				Hash,
				BlockNumber
			>
		> {
			let dname: pallet_did_name::did_name::AsciiDidName<Runtime> = name.try_into().ok()?;
			pallet_did_name::Owner::<Runtime>::get(&dname)
				.and_then(|owner_info| {
					pallet_did::Did::<Runtime>::get(&owner_info.owner).map(|details| (owner_info, details))
				})
				.map(|(owner_info, details)| {
					let service_endpoints = pallet_did::ServiceEndpoints::<Runtime>::iter_prefix(&owner_info.owner).map(|e| From::from(e.1)).collect();

					pallet_did_runtime_api::RawDidLinkedInfo{
						identifier: owner_info.owner.clone(),
						account: owner_info.owner,
						name: Some(dname.into()),
						service_endpoints,
						details: details.into(),
					}
			})
		}
	}

	impl pallet_transaction_weight_runtime_api::TransactionWeightApi<Block> for Runtime {
		fn query_weight_info(uxt: <Block as BlockT>::Extrinsic) -> RuntimeDispatchWeightInfo {
			NetworkMembership::query_weight_info(uxt)
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
	fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			log::info!("try-runtime::on_runtime_upgrade cord.");
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, RuntimeBlockWeights::get().max_block)
		}

		fn execute_block(
			block: Block,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect,
		) -> Weight {
			// NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
			// have a backtrace here.
			Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{baseline, Benchmarking, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;

			use frame_system_benchmarking::Pallet as SystemBench;
			use baseline::Pallet as BaselineBench;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();
			(list, storage_info)
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig,
		) -> Result<
			Vec<frame_benchmarking::BenchmarkBatch>,
			sp_runtime::RuntimeString,
		> {
			use frame_support::traits::WhitelistedStorageKeys;
			use frame_benchmarking::{baseline, Benchmarking, BenchmarkBatch };
			use sp_storage::TrackedStorageKey;

			use frame_system_benchmarking::Pallet as SystemBench;
			use baseline::Pallet as BaselineBench;

			impl frame_system_benchmarking::Config for Runtime {}
			impl baseline::Config for Runtime {}

			let whitelist: Vec<TrackedStorageKey> = AllPalletsWithSystem::whitelisted_storage_keys();

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			add_benchmarks!(params, batches);
			Ok(batches)
		}
	}

	impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
		fn create_default_config() -> Vec<u8> {
			create_default_config::<RuntimeGenesisConfig>()
		}

		fn build_config(config: Vec<u8>) -> sp_genesis_builder::Result {
			build_config::<RuntimeGenesisConfig>(config)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use frame_system::offchain::CreateSignedTransaction;

	#[test]
	fn validate_transaction_submitter_bounds() {
		fn is_submit_signed_transaction<T>()
		where
			T: CreateSignedTransaction<RuntimeCall>,
		{
		}

		is_submit_signed_transaction::<Runtime>();
	}
	#[test]
	fn call_size() {
		let size = core::mem::size_of::<RuntimeCall>();
		assert!(
			size <= CALL_PARAMS_MAX_SIZE,
			"size of RuntimeCall {} is more than {CALL_PARAMS_MAX_SIZE} bytes.
			 Some calls have too big arguments, use Box to reduce the size of RuntimeCall.
			 If the limit is too strong, maybe consider increase the limit.",
			size,
		);
	}
}
