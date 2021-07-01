// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # Vesting Pallet
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! A simple pallet providing a means of placing a linear curve on an account's locked balance. This
//! pallet ensures that there is a lock in place preventing the balance to drop below the *unvested*
//! amount for any reason other than transaction fee payment.
//!
//! As the amount vested increases over time, the amount unvested reduces. However, locks remain in
//! place and explicit action is needed on behalf of the user to ensure that the amount locked is
//! equivalent to the amount remaining to be vested. This is done through a dispatchable function,
//! either `vest` (in typical case where the sender is calling on their own behalf) or `vest_other`
//! in case the sender is calling on another account's behalf.
//!
//! ## Interface
//!
//! This pallet implements the `VestingSchedule` trait.
//!
//! ### Dispatchable Functions
//!
//! - `vest` - Update the lock, reducing it in line with the amount "vested" so far.
//! - `vest_other` - Update the lock of another account, reducing it in line with the amount
//!   "vested" so far.

#![cfg_attr(not(feature = "std"), no_std)]

mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
mod vesting_info;

pub mod weights;

use codec::{Decode, Encode};
use frame_support::{
	ensure,
	pallet_prelude::*,
	traits::{
		Currency, ExistenceRequirement, Get, LockIdentifier, LockableCurrency, VestingSchedule,
		WithdrawReasons,
	},
};
use frame_system::{ensure_root, ensure_signed, pallet_prelude::*};
pub use pallet::*;
use sp_runtime::{
	traits::{
		AtLeast32BitUnsigned, Bounded, CheckedDiv, Convert, MaybeSerializeDeserialize, One,
		Saturating, StaticLookup, Zero,
	},
	RuntimeDebug,
};
use sp_std::{convert::TryInto, fmt::Debug, prelude::*};
pub use vesting_info::*;
pub use weights::WeightInfo;

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type MaxLocksOf<T> =
	<<T as Config>::Currency as LockableCurrency<<T as frame_system::Config>::AccountId>>::MaxLocks;

const VESTING_ID: LockIdentifier = *b"vesting ";
const LOG_TARGET: &'static str = "runtime::vesting";

/// Actions to take on a users `Vesting` storage entry.
enum VestingAction {
	/// Do not actively remove any schedules.
	Passive,
	/// Remove the schedules specified by the index.
	Remove(usize),
	/// Remove the two schedules, specified by index, so they can be merged.
	Merge(usize, usize),
}

impl VestingAction {
	/// Whether or not the filter says the schedule index should be removed.
	fn should_remove(&self, index: &usize) -> bool {
		match self {
			Self::Passive => false,
			Self::Remove(index1) => index1 == index,
			Self::Merge(index1, index2) => index1 == index || index2 == index,
		}
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The currency trait.
		type Currency: LockableCurrency<Self::AccountId>;

		/// Convert the block number into a balance.
		type BlockNumberToBalance: Convert<Self::BlockNumber, BalanceOf<Self>>;

		/// The minimum amount transferred to call `vested_transfer`.
		#[pallet::constant]
		type MinVestedTransfer: Get<BalanceOf<Self>>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Maximum number of vesting schedules an account may have at a given moment.
		#[pallet::constant]
		type MaxVestingSchedules: Get<u32>;
	}

	/// Information regarding the vesting of a given account.
	#[pallet::storage]
	#[pallet::getter(fn vesting)]
	pub type Vesting<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		T::AccountId,
		BoundedVec<VestingInfo<BalanceOf<T>, T::BlockNumber>, T::MaxVestingSchedules>
	>;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub vesting: Vec<(T::AccountId, T::BlockNumber, T::BlockNumber, BalanceOf<T>)>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			GenesisConfig {
				vesting: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			use sp_runtime::traits::Saturating;

			// Generate initial vesting configuration
			// * who - Account which we are generating vesting configuration for
			// * begin - Block when the account will start to vest
			// * length - Number of blocks from `begin` until fully vested
			// * liquid - Number of units which can be spent before vesting begins
			for &(ref who, begin, length, liquid) in self.vesting.iter() {
				let balance = T::Currency::free_balance(who);
				assert!(!balance.is_zero(), "Currencies must be init'd before vesting");
				// Total genesis `balance` minus `liquid` equals funds locked for vesting
				let locked = balance.saturating_sub(liquid);
				let length_as_balance = T::BlockNumberToBalance::convert(length);
				let per_block = locked / length_as_balance.max(sp_runtime::traits::One::one());
				let vesting_info = VestingInfo::new::<T>(locked, per_block, begin);
				vesting_info.validate::<T::BlockNumberToBalance, T>()
					.expect("Invalid VestingInfo params at genesis");

				Vesting::<T>::try_append(who, vesting_info)
					.expect("Too many vesting schedules at genesis.");
				let reasons = WithdrawReasons::TRANSFER | WithdrawReasons::RESERVE;
				T::Currency::set_lock(VESTING_ID, who, locked, reasons);
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	#[pallet::metadata(
		T::AccountId = "AccountId", BalanceOf<T> = "Balance", T::BlockNumber = "BlockNumber"
	)]
	pub enum Event<T: Config> {
		/// The amount vested has been updated. This could indicate more funds are available. The
		/// balance given is the amount which is left unvested (and thus locked).
		/// \[account, unvested\]
		VestingUpdated(T::AccountId, BalanceOf<T>),
		/// An \[account\] has become fully vested. No further vesting can happen.
		VestingCompleted(T::AccountId),
		/// 2 vesting schedules where successfully merged together and the merged schedule was added.
		/// \[locked, per_block, starting_block\]
		MergedScheduleAdded(BalanceOf<T>, BalanceOf<T>, T::BlockNumber),
	}

	/// Error for the vesting pallet.
	#[pallet::error]
	pub enum Error<T> {
		/// The account given is not vesting.
		NotVesting,
		/// The account already has `MaxVestingSchedules` number of schedules and thus
		/// cannot add another one. Consider merging existing schedules in order to add another.
		AtMaxVestingSchedules,
		/// Amount being transferred is too low to create a vesting schedule.
		AmountLow,
		/// At least one of the indexes is out of bounds of the vesting schedules.
		ScheduleIndexOutOfBounds,
		/// Failed to create a new schedule because some parameters where invalid. e.g. `locked` was 0.
		InvalidScheduleParams,
		/// A schedule contained a `per_block` of 0 or `locked / per_block > BlockNumber::max_value()`,
		/// thus rendering it unable to ever fully unlock funds.
		InfiniteSchedule,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Unlock any vested funds of the sender account.
		///
		/// The dispatch origin for this call must be _Signed_ and the sender must have funds still
		/// locked under this pallet.
		///
		/// Emits either `VestingCompleted` or `VestingUpdated`.
		///
		/// # <weight>
		/// - `O(1)`.
		/// - DbWeight: 2 Reads, 2 Writes
		///     - Reads: Vesting Storage, Balances Locks, [Sender Account]
		///     - Writes: Vesting Storage, Balances Locks, [Sender Account]
		/// # </weight>
		#[pallet::weight(T::WeightInfo::vest_locked(MaxLocksOf::<T>::get())
			.max(T::WeightInfo::vest_unlocked(MaxLocksOf::<T>::get()))
		)]
		pub fn vest(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			Self::do_vest(who)
		}

		/// Unlock any vested funds of a `target` account.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// - `target`: The account whose vested funds should be unlocked. Must have funds still
		/// locked under this pallet.
		///
		/// Emits either `VestingCompleted` or `VestingUpdated`.
		///
		/// # <weight>
		/// - `O(1)`.
		/// - DbWeight: 3 Reads, 3 Writes
		///     - Reads: Vesting Storage, Balances Locks, Target Account
		///     - Writes: Vesting Storage, Balances Locks, Target Account
		/// # </weight>
		#[pallet::weight(T::WeightInfo::vest_other_locked(MaxLocksOf::<T>::get())
			.max(T::WeightInfo::vest_other_unlocked(MaxLocksOf::<T>::get()))
		)]
		pub fn vest_other(
			origin: OriginFor<T>,
			target: <T::Lookup as StaticLookup>::Source,
		) -> DispatchResult {
			ensure_signed(origin)?;
			let who = T::Lookup::lookup(target)?;
			Self::do_vest(who)
		}

		/// Create a vested transfer.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// - `target`: The account receiving the vested funds.
		/// - `schedule`: The vesting schedule attached to the transfer.
		///
		/// Emits `VestingCreated`.
		///
		/// # <weight>
		/// - `O(1)`.
		/// - DbWeight: 3 Reads, 3 Writes
		///     - Reads: Vesting Storage, Balances Locks, Target Account, [Sender Account]
		///     - Writes: Vesting Storage, Balances Locks, Target Account, [Sender Account]
		/// # </weight>
		#[pallet::weight(
			T::WeightInfo::last_vested_transfer(MaxLocksOf::<T>::get())
			.max(T::WeightInfo::first_vested_transfer(MaxLocksOf::<T>::get()))
		)]
		pub fn vested_transfer(
			origin: OriginFor<T>,
			target: <T::Lookup as StaticLookup>::Source,
			schedule: VestingInfo<BalanceOf<T>, T::BlockNumber>,
		) -> DispatchResult {
			let transactor = ensure_signed(origin)?;
			let transactor = <T::Lookup as StaticLookup>::unlookup(transactor);
			Self::do_vested_transfer(transactor, target, schedule)
		}

		/// Force a vested transfer.
		///
		/// The dispatch origin for this call must be _Root_.
		///
		/// - `source`: The account whose funds should be transferred.
		/// - `target`: The account that should be transferred the vested funds.
		/// - `schedule`: The vesting schedule attached to the transfer.
		///
		/// Emits `VestingCreated`.
		///
		/// # <weight>
		/// - `O(1)`.
		/// - DbWeight: 4 Reads, 4 Writes
		///     - Reads: Vesting Storage, Balances Locks, Target Account, Source Account
		///     - Writes: Vesting Storage, Balances Locks, Target Account, Source Account
		/// # </weight>
		#[pallet::weight(
			T::WeightInfo::first_force_vested_transfer(MaxLocksOf::<T>::get())
			.max(T::WeightInfo::last_force_vested_transfer(MaxLocksOf::<T>::get()))
		)]
		pub fn force_vested_transfer(
			origin: OriginFor<T>,
			source: <T::Lookup as StaticLookup>::Source,
			target: <T::Lookup as StaticLookup>::Source,
			schedule: VestingInfo<BalanceOf<T>, T::BlockNumber>,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::do_vested_transfer(source, target, schedule)
		}

		/// Merge two vesting schedules together, creating a new vesting schedule that unlocks over
		/// highest possible start and end blocks. If both schedules have already started the current
		/// block will be used as the schedule start; with the caveat that if one schedule is finished
		/// by the current block, the other will be treated as the new merged schedule, unmodified.
		///
		/// NOTE: If `schedule1_index == schedule2_index` this is a no-op.
		/// NOTE: This will unlock all schedules through the current block prior to merging.
		/// NOTE: If both schedules have ended by the current block, no new schedule will be created
		/// and both will be removed.
		///
		/// Merged schedule attributes:
		/// - `starting_block`: `MAX(schedule1.starting_block, scheduled2.starting_block, current_block)`.
		/// - `ending_block`: `MAX(schedule1.ending_block, schedule2.ending_block)`.
		/// - `locked`: `schedule1.locked_at(current_block) + schedule2.locked_at(current_block)`.
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// - `schedule1_index`: index of the first schedule to merge.
		/// - `schedule2_index`: index of the second schedule to merge.
		#[pallet::weight(
			T::WeightInfo::not_unlocking_merge_schedules(MaxLocksOf::<T>::get())
			.max(T::WeightInfo::unlocking_merge_schedules(MaxLocksOf::<T>::get()))
		)]
		pub fn merge_schedules(
			origin: OriginFor<T>,
			schedule1_index: u32,
			schedule2_index: u32,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			if schedule1_index == schedule2_index {
				return Ok(());
			};
			let schedule1_index = schedule1_index as usize;
			let schedule2_index = schedule2_index as usize;
			let vesting = Self::vesting(&who).ok_or(Error::<T>::NotVesting)?;

			// The schedule index is based off of the schedule ordering prior to filtering out any
			// schedules that may be ending at this block.
			let schedule1 =
				*vesting.get(schedule1_index).ok_or(Error::<T>::ScheduleIndexOutOfBounds)?;
			let schedule2 =
				*vesting.get(schedule2_index).ok_or(Error::<T>::ScheduleIndexOutOfBounds)?;
			let merge_action = VestingAction::Merge(schedule1_index, schedule2_index);

			// The length of vesting decreases by 2 here since wem filter out 2 schedules. Thus we know
			// below that we can safely insert the new merged schedule.
			let (mut schedules, mut locked_now) =
				Self::report_schedule_updates(vesting, merge_action);

			let now = <frame_system::Pallet<T>>::block_number();
			if let Some(new_schedule) = Self::merge_vesting_info(now, schedule1, schedule2)? {
				// Merging created a new schedule; so now we need to add it to the
				// accounts vesting schedules,
				if let Err(_) = schedules.try_push(new_schedule) {
					// (It shouldn't be possible for this to fail because we removed 2 schedules above.)
					log::warn!(
						target: LOG_TARGET,
						"faulty logic led to attempting to add too many vesting schedules",
					);
					return Err(Error::<T>::AtMaxVestingSchedules.into());
				}
				// (We use `locked_at` in case this is a schedule that started in the past.)
				let new_schedule_locked = new_schedule.locked_at::<T::BlockNumberToBalance>(now);
				// update the locked amount to reflect the schedule we just added,
				locked_now = locked_now.saturating_add(new_schedule_locked);
				// and deposit an event to show the new, new, merged schedule.
				Self::deposit_event(Event::<T>::MergedScheduleAdded(
					new_schedule.locked(),
					new_schedule.per_block(),
					new_schedule.starting_block(),
				));
			} // In the None case there was no new schedule to account for.

			debug_assert!(schedules.len() <= T::MaxVestingSchedules::get() as usize);
			debug_assert!(
				locked_now > Zero::zero() && schedules.len() > 0 ||
					locked_now == Zero::zero() && schedules.len() == 0
			);
			if let Err(e) = Self::write_vesting(&who, schedules) {
				// The write should not fail because that would mean their where too
				// many schedules to start out with.
				log::warn!(target: LOG_TARGET, "an account has too many vesting schedules",);
				return e.into();
			};
			Self::write_lock(&who, locked_now);

			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	// Create a new `VestingInfo`, based off of two other `VestingInfo`s.
	// NOTE: We assume both schedules have had funds unlocked up through the current block.
	fn merge_vesting_info(
		now: T::BlockNumber,
		schedule1: VestingInfo<BalanceOf<T>, T::BlockNumber>,
		schedule2: VestingInfo<BalanceOf<T>, T::BlockNumber>,
	) -> Result<Option<VestingInfo<BalanceOf<T>, T::BlockNumber>>, DispatchError> {
		let schedule1_ending_block = schedule1.ending_block::<T::BlockNumberToBalance, T>()?;
		let schedule2_ending_block = schedule2.ending_block::<T::BlockNumberToBalance, T>()?;
		let now_as_balance = T::BlockNumberToBalance::convert(now);

		// Check if one or both schedules have ended.
		match (schedule1_ending_block <= now_as_balance, schedule2_ending_block <= now_as_balance) {
			// If both schedules have ended, we don't merge and exit early.
			(true, true) => return Ok(None),
			// If one schedule has ended, we treat the one that has not ended as the new
			// merged schedule.
			(true, false) => return Ok(Some(schedule2)),
			(false, true) => return Ok(Some(schedule1)),
			// If neither schedule has ended don't exit early.
			_ => {}
		}

		let locked = schedule1
			.locked_at::<T::BlockNumberToBalance>(now)
			.saturating_add(schedule2.locked_at::<T::BlockNumberToBalance>(now));
		// This shouldn't happen because we know at least one ending block is greater than now.
		if locked.is_zero() {
			log::warn!(
				target: LOG_TARGET,
				"merge_vesting_info validation checks failed to catch a locked of 0"
			);
			return Ok(None);
		}

		let ending_block = schedule1_ending_block.max(schedule2_ending_block);
		let starting_block = now.max(schedule1.starting_block()).max(schedule2.starting_block());
		let duration =
			ending_block.saturating_sub(T::BlockNumberToBalance::convert(starting_block));
		let per_block = if duration > locked {
			// This would mean we have a per_block of less than 1, which should not be not possible
			// because when we create the new schedule is at most the same duration as the longest,
			// but never greater.
			One::one()
		} else {
			match locked.checked_div(&duration) {
				// The logic of `ending_block` guarantees that each schedule ends at least a block
				// after it starts and since we take the max starting and ending_block we should never
				// get here
				None => locked,
				Some(per_block) => per_block,
			}
		};

		// At this point inputs have been validated, so this should always be `Some`.
		let schedule = VestingInfo::new::<T>(locked, per_block, starting_block);
		debug_assert!(schedule.validate::<T::BlockNumberToBalance, T>().is_ok());

		Ok(Some(schedule))
	}

	// Execute a vested transfer from `source` to `target` with the given `schedule`.
	fn do_vested_transfer(
		source: <T::Lookup as StaticLookup>::Source,
		target: <T::Lookup as StaticLookup>::Source,
		schedule: VestingInfo<BalanceOf<T>, T::BlockNumber>,
	) -> DispatchResult {
		ensure!(schedule.locked() > T::MinVestedTransfer::get(), Error::<T>::AmountLow);
		schedule.validate::<T::BlockNumberToBalance, T>()?;
		let schedule = schedule.correct();

		let target = T::Lookup::lookup(target)?;
		let source = T::Lookup::lookup(source)?;
		ensure!(
			Vesting::<T>::decode_len(&target).unwrap_or_default() <
				T::MaxVestingSchedules::get() as usize,
			Error::<T>::AtMaxVestingSchedules
		);

		T::Currency::transfer(
			&source,
			&target,
			schedule.locked(),
			ExistenceRequirement::AllowDeath,
		)?;

		// We can't let this fail because the currency transfer has already happened
		Self::add_vesting_schedule(
			&target,
			schedule.locked(),
			schedule.per_block(),
			schedule.starting_block(),
		)
		.expect("schedule inputs and vec bounds have been validated. q.e.d.");

		Ok(())
	}

	/// Iterate through the schedules to track the current locked amount and
	/// filter out completed and specified schedules.
	///
	/// Returns a tuple that consists of:
	/// - bounded vec of vesting schedules, where completed schedules and those
	/// specified by filter are remove.
	/// - the amount locked at the current block number based on the given schedules.
	///
	/// NOTE: the amount locked does not include any schedules that are filtered out.
	fn report_schedule_updates(
		schedules: BoundedVec<VestingInfo<BalanceOf<T>, T::BlockNumber>, T::MaxVestingSchedules>,
		action: VestingAction,
	) -> (BoundedVec<VestingInfo<BalanceOf<T>, T::BlockNumber>, T::MaxVestingSchedules>, BalanceOf<T>)
	{
		let now = <frame_system::Pallet<T>>::block_number();

		let mut total_locked_now: BalanceOf<T> = Zero::zero();
		let filtered_schedules = schedules
			.into_iter()
			.enumerate()
			.filter_map(|(index, schedule)| {
				let locked_now = schedule.locked_at::<T::BlockNumberToBalance>(now);
				if locked_now.is_zero() || action.should_remove(&index) {
					None
				} else {
					// We track the locked amount only if the schedule is included.
					total_locked_now = total_locked_now.saturating_add(locked_now);
					Some(schedule)
				}
			})
			.collect::<Vec<_>>()
			.try_into()
			.expect("`BoundedVec` is created from another `BoundedVec` with same bound; q.e.d.");

		(filtered_schedules, total_locked_now)
	}

	/// Write an accounts updated vesting lock to storage.
	fn write_lock(who: &T::AccountId, total_locked_now: BalanceOf<T>) {
		if total_locked_now.is_zero() {
			T::Currency::remove_lock(VESTING_ID, who);
			Self::deposit_event(Event::<T>::VestingCompleted(who.clone()));
		} else {
			let reasons = WithdrawReasons::TRANSFER | WithdrawReasons::RESERVE;
			T::Currency::set_lock(VESTING_ID, who, total_locked_now, reasons);
			Self::deposit_event(Event::<T>::VestingUpdated(who.clone(), total_locked_now));
		};
	}

	/// Write an accounts updated vesting schedules to storage.
	fn write_vesting(
		who: &T::AccountId,
		schedules: BoundedVec<VestingInfo<BalanceOf<T>, T::BlockNumber>, T::MaxVestingSchedules>,
	) -> Result<(), DispatchError> {
		if schedules.len() > T::MaxVestingSchedules::get() as usize {
			return Err(Error::<T>::AtMaxVestingSchedules.into());
		} else if schedules.len() == 0 {
			Vesting::<T>::remove(&who);
		} else {
			Vesting::<T>::insert(who, schedules)
		}

		Ok(())
	}

	/// Unlock any vested funds of `who`.
	fn do_vest(who: T::AccountId) -> DispatchResult {
		let schedules = Self::vesting(&who).ok_or(Error::<T>::NotVesting)?;

		let (schedules, locked_now) =
			Self::report_schedule_updates(schedules, VestingAction::Passive);
		debug_assert!(schedules.len() <= T::MaxVestingSchedules::get() as usize);
		debug_assert!(
			locked_now > Zero::zero() && schedules.len() > 0 ||
				locked_now == Zero::zero() && schedules.len() == 0
		);

		if let Err(e) = Self::write_vesting(&who, schedules) {
			// The write should not fail because that would mean their where too
			// many schedules to start out with.
			log::warn!(target: LOG_TARGET, "an account has too many vesting schedules",);
			return e.into();
		};
		Self::write_lock(&who, locked_now);

		Ok(())
	}
}

impl<T: Config> VestingSchedule<T::AccountId> for Pallet<T>
where
	BalanceOf<T>: MaybeSerializeDeserialize + Debug,
{
	type Currency = T::Currency;
	type Moment = T::BlockNumber;

	/// Get the amount that is currently being vested and cannot be transferred out of this account.
	fn vesting_balance(who: &T::AccountId) -> Option<BalanceOf<T>> {
		if let Some(v) = Self::vesting(who) {
			let now = <frame_system::Pallet<T>>::block_number();
			let total_locked_now = v.iter().fold(Zero::zero(), |total, schedule| {
				schedule.locked_at::<T::BlockNumberToBalance>(now).saturating_add(total)
			});
			Some(T::Currency::free_balance(who).min(total_locked_now))
		} else {
			None
		}
	}

	/// Adds a vesting schedule to a given account.
	///
	/// If the account has `MaxVestingSchedules`, an Error is returned and nothing
	/// is updated.
	///
	/// On success, a linearly reducing amount of funds will be locked. In order to realise any
	/// reduction of the lock over time as it diminishes, the account owner must use `vest` or
	/// `vest_other`.
	///
	/// Is a no-op if the amount to be vested is zero.
	/// NOTE: it is assumed the function user has done necessary `VestingInfo` param validation.
	fn add_vesting_schedule(
		who: &T::AccountId,
		locked: BalanceOf<T>,
		per_block: BalanceOf<T>,
		starting_block: T::BlockNumber,
	) -> DispatchResult {
		if locked.is_zero() {
			return Ok(());
		}

		let vesting_schedule = VestingInfo::new::<T>(locked, per_block, starting_block);
		let mut schedules = Self::vesting(who).unwrap_or_default();

		// NOTE: we must push the new schedule so that `report_schedule_updates`
		// will give the correct new locked amount.
		ensure!(schedules.try_push(vesting_schedule).is_ok(), Error::<T>::AtMaxVestingSchedules);

		let (schedules, locked_now) =
			Self::report_schedule_updates(schedules, VestingAction::Passive);
		debug_assert!(schedules.len() <= T::MaxVestingSchedules::get() as usize);
		debug_assert!(
			locked_now > Zero::zero() && schedules.len() > 0 ||
				locked_now == Zero::zero() && schedules.len() == 0
		);

		if let Err(e) = Self::write_vesting(&who, schedules) {
			// The write should not fail because that would mean their where too
			// many schedules to start out with.
			log::warn!(target: LOG_TARGET, "an account has too many vesting schedules",);
			return e.into();
		};
		Self::write_lock(who, locked_now);

		Ok(())
	}

	/// Remove a vesting schedule for a given account. Will error if `schedule_index` is `None`.
	fn remove_vesting_schedule(who: &T::AccountId, schedule_index: u32) -> DispatchResult {
		let remove_action = VestingAction::Remove(schedule_index as usize);
		let schedules = Self::vesting(who).ok_or(Error::<T>::NotVesting)?;

		let (schedules, locked_now) = Self::report_schedule_updates(schedules, remove_action);
		debug_assert!(schedules.len() <= T::MaxVestingSchedules::get() as usize);
		debug_assert!(
			locked_now > Zero::zero() && schedules.len() > 0 ||
				locked_now == Zero::zero() && schedules.len() == 0
		);

		if let Err(e) = Self::write_vesting(&who, schedules) {
			// The write should not fail because that would mean their where too
			// many schedules to start out with.
			log::warn!(target: LOG_TARGET, "an account has too many vesting schedules",);
			return e.into();
		};
		Self::write_lock(who, locked_now);
		Ok(())
	}
}
