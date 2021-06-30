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

use frame_support::{assert_noop, assert_ok, assert_storage_noop};
use frame_system::RawOrigin;
use sp_runtime::traits::{BadOrigin, Identity};

use super::*;
use crate::mock::{Balances, ExtBuilder, System, Test};

/// A default existential deposit.
const ED: u64 = 256;

#[test]
fn check_vesting_status() {
	ExtBuilder::default()
		.existential_deposit(256)
		.build()
		.execute_with(|| {
			let user1_free_balance = Balances::free_balance(&1);
			let user2_free_balance = Balances::free_balance(&2);
			let user12_free_balance = Balances::free_balance(&12);
			assert_eq!(user1_free_balance, 256 * 10); // Account 1 has free balance
			assert_eq!(user2_free_balance, 256 * 20); // Account 2 has free balance
			assert_eq!(user12_free_balance, 256 * 10); // Account 12 has free balance
			let user1_vesting_schedule = VestingInfo::new::<Test>(
				256 * 5,
				128, // Vesting over 10 blocks
				0,
			);
			let user2_vesting_schedule = VestingInfo::new::<Test>(
				256 * 20,
				256, // Vesting over 20 blocks
				10,
			);
			let user12_vesting_schedule = VestingInfo::new::<Test>(
				256 * 5,
				64, // Vesting over 20 blocks
				10,
			);
			assert_eq!(mock::Vesting::vesting(&1).unwrap(), vec![user1_vesting_schedule]); // Account 1 has a vesting schedule
			assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![user2_vesting_schedule]); // Account 2 has a vesting schedule
			assert_eq!(mock::Vesting::vesting(&12).unwrap(), vec![user12_vesting_schedule]); // Account 12 has a vesting schedule

			// Account 1 has only 128 units vested from their illiquid 256 * 5 units at block 1
			assert_eq!(mock::Vesting::vesting_balance(&1), Some(128 * 9));
			// Account 2 has their full balance locked
			assert_eq!(mock::Vesting::vesting_balance(&2), Some(user2_free_balance));
			// Account 12 has only their illiquid funds locked
			assert_eq!(mock::Vesting::vesting_balance(&12), Some(user12_free_balance - 256 * 5));

			System::set_block_number(10);
			assert_eq!(System::block_number(), 10);

			// Account 1 has fully vested by block 10
			assert_eq!(mock::Vesting::vesting_balance(&1), Some(0));
			// Account 2 has started vesting by block 10
			assert_eq!(mock::Vesting::vesting_balance(&2), Some(user2_free_balance));
			// Account 12 has started vesting by block 10
			assert_eq!(mock::Vesting::vesting_balance(&12), Some(user12_free_balance - 256 * 5));

			System::set_block_number(30);
			assert_eq!(System::block_number(), 30);

			assert_eq!(mock::Vesting::vesting_balance(&1), Some(0)); // Account 1 is still fully vested, and not negative
			assert_eq!(mock::Vesting::vesting_balance(&2), Some(0)); // Account 2 has fully vested by block 30
			assert_eq!(mock::Vesting::vesting_balance(&12), Some(0)); // Account 2 has fully vested by block 30

			// Once we unlock the funds, they are removed from storage.
			assert_ok!(mock::Vesting::vest(Some(1).into()));
			assert!(!<Vesting<Test>>::contains_key(&1));
			assert_ok!(mock::Vesting::vest(Some(2).into()));
			assert!(!<Vesting<Test>>::contains_key(&2));
			assert_ok!(mock::Vesting::vest(Some(12).into()));
			assert!(!<Vesting<Test>>::contains_key(&12));
		});
}

#[test]
fn check_vesting_status_for_multi_schedule_account() {
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		assert_eq!(System::block_number(), 1);
		let sched0 = VestingInfo::new::<Test>(
			ED * 20,
			ED, // Vesting over 20 blocks
			10,
		);
		// Account 2 already has a vesting schedule.
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0]);

		// Account 2's free balance is from sched0
		let free_balance = Balances::free_balance(&2);
		assert_eq!(free_balance, ED * (20));
		assert_eq!(mock::Vesting::vesting_balance(&2), Some(free_balance));

		// Add a 2nd schedule that is already unlocking by block #1
		let sched1 = VestingInfo::new::<Test>(
			ED * 10,
			ED, // Vesting over 10 blocks
			0,
		);
		assert_ok!(mock::Vesting::vested_transfer(Some(4).into(), 2, sched1));
		// Free balance is equal to the two existing schedules total amount.
		let free_balance = Balances::free_balance(&2);
		assert_eq!(free_balance, ED * (10 + 20));
		// The most recently added schedule exists.
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0, sched1]);
		// sched1 has free funds at block #1, but nothing else.
		assert_eq!(mock::Vesting::vesting_balance(&2), Some(free_balance - sched1.per_block()));

		// Add a 3rd schedule
		let sched2 = VestingInfo::new::<Test>(
			ED * 30,
			ED, // Vesting over 30 blocks
			5,
		);
		assert_ok!(mock::Vesting::vested_transfer(Some(4).into(), 2, sched2));

		System::set_block_number(9);
		// Free balance is equal to the 3 existing schedules total amount.
		let free_balance = Balances::free_balance(&2);
		assert_eq!(free_balance, ED * (10 + 20 + 30));
		// sched1 and sched2 are freeing funds at block #9.
		assert_eq!(
			mock::Vesting::vesting_balance(&2),
			Some(free_balance - sched1.per_block() * 9 - sched2.per_block() * 4)
		);

		System::set_block_number(20);
		// At block #20 sched1 is fully unlocked while sched2 and sched0 are partially unlocked.
		assert_eq!(
			mock::Vesting::vesting_balance(&2),
			Some(
				free_balance - sched1.locked() - sched2.per_block() * 15 - sched0.per_block() * 10
			)
		);

		System::set_block_number(30);
		// At block #30 sched0 and sched1 are fully unlocked while sched2 is partially unlocked.
		assert_eq!(
			mock::Vesting::vesting_balance(&2),
			Some(free_balance - sched1.locked() - sched2.per_block() * 25 - sched0.locked())
		);

		// At block #35 sched2 fully unlocks and thus all schedules funds are unlocked.
		System::set_block_number(35);
		assert_eq!(System::block_number(), 35);
		assert_eq!(mock::Vesting::vesting_balance(&2), Some(0));
		// Since we have not called any extrinsics that would unlock funds the schedules
		// are still in storage.
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0, sched1, sched2]);
		// But once we unlock the funds, they are removed from storage.
		assert_ok!(mock::Vesting::vest(Some(2).into()));
		assert!(!<Vesting<Test>>::contains_key(&2));
	});
}

#[test]
fn unvested_balance_should_not_transfer() {
	ExtBuilder::default()
		.existential_deposit(10)
		.build()
		.execute_with(|| {
			let user1_free_balance = Balances::free_balance(&1);
			assert_eq!(user1_free_balance, 100); // Account 1 has free balance
			// Account 1 has only 5 units vested at block 1 (plus 50 unvested)
			assert_eq!(mock::Vesting::vesting_balance(&1), Some(45));
			assert_noop!(
				Balances::transfer(Some(1).into(), 2, 56),
				pallet_balances::Error::<Test, _>::LiquidityRestrictions,
			); // Account 1 cannot send more than vested amount
		});
}

#[test]
fn vested_balance_should_transfer() {
	ExtBuilder::default()
		.existential_deposit(10)
		.build()
		.execute_with(|| {
			let user1_free_balance = Balances::free_balance(&1);
			assert_eq!(user1_free_balance, 100); // Account 1 has free balance
			// Account 1 has only 5 units vested at block 1 (plus 50 unvested)
			assert_eq!(mock::Vesting::vesting_balance(&1), Some(45));
			assert_ok!(mock::Vesting::vest(Some(1).into()));
			assert_ok!(Balances::transfer(Some(1).into(), 2, 55));
		});
}

#[test]
fn vested_balance_should_transfer_using_vest_other() {
	ExtBuilder::default()
		.existential_deposit(10)
		.build()
		.execute_with(|| {
			let user1_free_balance = Balances::free_balance(&1);
			assert_eq!(user1_free_balance, 100); // Account 1 has free balance
			// Account 1 has only 5 units vested at block 1 (plus 50 unvested)
			assert_eq!(mock::Vesting::vesting_balance(&1), Some(45));
			assert_ok!(mock::Vesting::vest_other(Some(2).into(), 1));
			assert_ok!(Balances::transfer(Some(1).into(), 2, 55));
		});
}

#[test]
fn extra_balance_should_transfer() {
	ExtBuilder::default()
		.existential_deposit(10)
		.build()
		.execute_with(|| {
			assert_ok!(Balances::transfer(Some(3).into(), 1, 100));
			assert_ok!(Balances::transfer(Some(3).into(), 2, 100));

			let user1_free_balance = Balances::free_balance(&1);
			assert_eq!(user1_free_balance, 200); // Account 1 has 100 more free balance than normal

			let user2_free_balance = Balances::free_balance(&2);
			assert_eq!(user2_free_balance, 300); // Account 2 has 100 more free balance than normal

			// Account 1 has only 5 units vested at block 1 (plus 150 unvested)
			assert_eq!(mock::Vesting::vesting_balance(&1), Some(45));
			assert_ok!(mock::Vesting::vest(Some(1).into()));
			assert_ok!(Balances::transfer(Some(1).into(), 3, 155)); // Account 1 can send extra units gained

			// Account 2 has no units vested at block 1, but gained 100
			assert_eq!(mock::Vesting::vesting_balance(&2), Some(200));
			assert_ok!(mock::Vesting::vest(Some(2).into()));
			assert_ok!(Balances::transfer(Some(2).into(), 3, 100)); // Account 2 can send extra units gained
		});
}

#[test]
fn liquid_funds_should_transfer_with_delayed_vesting() {
	ExtBuilder::default()
		.existential_deposit(256)
		.build()
		.execute_with(|| {
			let user12_free_balance = Balances::free_balance(&12);

			assert_eq!(user12_free_balance, 2560); // Account 12 has free balance
			// Account 12 has liquid funds
			assert_eq!(mock::Vesting::vesting_balance(&12), Some(user12_free_balance - 256 * 5));

			// Account 12 has delayed vesting
			let user12_vesting_schedule = VestingInfo::new::<Test>(
				256 * 5,
				64, // Vesting over 20 blocks
				10,
			);
			assert_eq!(mock::Vesting::vesting(&12).unwrap(), vec![user12_vesting_schedule]);

			// Account 12 can still send liquid funds
			assert_ok!(Balances::transfer(Some(12).into(), 3, 256 * 5));
		});
}

#[test]
fn vested_transfer_works() {
	ExtBuilder::default()
		.existential_deposit(256)
		.build()
		.execute_with(|| {
			let user3_free_balance = Balances::free_balance(&3);
			let user4_free_balance = Balances::free_balance(&4);
			assert_eq!(user3_free_balance, 256 * 30);
			assert_eq!(user4_free_balance, 256 * 40);
			// Account 4 should not have any vesting yet.
			assert_eq!(mock::Vesting::vesting(&4), None);
			// Make the schedule for the new transfer.
			let new_vesting_schedule = VestingInfo::new::<Test>(
				256 * 5,
				64, // Vesting over 20 blocks
				10,
			);
			assert_ok!(mock::Vesting::vested_transfer(Some(3).into(), 4, new_vesting_schedule));
			// Now account 4 should have vesting.
			assert_eq!(mock::Vesting::vesting(&4).unwrap(), vec![new_vesting_schedule]);
			// Ensure the transfer happened correctly.
			let user3_free_balance_updated = Balances::free_balance(&3);
			assert_eq!(user3_free_balance_updated, 256 * 25);
			let user4_free_balance_updated = Balances::free_balance(&4);
			assert_eq!(user4_free_balance_updated, 256 * 45);
			// Account 4 has 5 * 256 locked.
			assert_eq!(mock::Vesting::vesting_balance(&4), Some(256 * 5));

			System::set_block_number(20);
			assert_eq!(System::block_number(), 20);

			// Account 4 has 5 * 64 units vested by block 20.
			assert_eq!(mock::Vesting::vesting_balance(&4), Some(10 * 64));

			System::set_block_number(30);
			assert_eq!(System::block_number(), 30);

			// Account 4 has fully vested
			assert_eq!(mock::Vesting::vesting_balance(&4), Some(0));
			// and after unlocking its schedules are removed from storage.
			assert_ok!(mock::Vesting::vest(Some(4).into()));
			assert!(!<Vesting<Test>>::contains_key(&4));
		});
}

#[test]
fn vested_transfer_correctly_fails() {
	ExtBuilder::default()
		.existential_deposit(256)
		.build()
		.execute_with(|| {
			let user2_free_balance = Balances::free_balance(&2);
			let user4_free_balance = Balances::free_balance(&4);
			assert_eq!(user2_free_balance, 256 * 20);
			assert_eq!(user4_free_balance, 256 * 40);

			// Account 2 should already have a vesting schedule.
			let user2_vesting_schedule = VestingInfo::new::<Test>(
				256 * 20,
				256, // Vesting over 20 blocks
				10,
			);
			assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![user2_vesting_schedule]);

			// Fails due to too low transfer amount.
			let new_vesting_schedule_too_low =
				VestingInfo::new::<Test>(<Test as Config>::MinVestedTransfer::get() - 1, 64, 10);
			assert_noop!(
				mock::Vesting::vested_transfer(Some(3).into(), 4, new_vesting_schedule_too_low),
				Error::<Test>::AmountLow,
			);

			// `per_block` is 0, which would result in a schedule with infinite duration.
			let schedule_per_block_0 = VestingInfo::new::<Test>(256, 0, 10);
			assert_noop!(
				mock::Vesting::force_vested_transfer(RawOrigin::Root.into(), 3, 4, schedule_per_block_0),
				Error::<Test>::InvalidScheduleParams,
			);

			// `locked` is 0.
			let schedule_locked_0 = VestingInfo::new::<Test>(0, 1, 10);
			assert_noop!(
				mock::Vesting::force_vested_transfer(RawOrigin::Root.into(), 3, 4, schedule_locked_0),
				Error::<Test>::AmountLow,
			);

			// Free balance has not changed.
			assert_eq!(user2_free_balance, Balances::free_balance(&2));
			assert_eq!(user4_free_balance, Balances::free_balance(&4));
		});
}

#[test]
fn vested_transfer_allows_max_schedules() {
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		let mut user_4_free_balance = Balances::free_balance(&4);
		let max_schedules = <Test as Config>::MaxVestingSchedules::get();
		let sched = VestingInfo::new::<Test>(
			ED, 1, // Vest over 256 blocks.
			10,
		);
		// Add max amount schedules to user 4.
		for _ in 0 .. max_schedules {
			assert_ok!(mock::Vesting::vested_transfer(Some(3).into(), 4, sched));
		}
		// The schedules count towards vesting balance
		let transferred_amount = ED * max_schedules as u64;
		assert_eq!(mock::Vesting::vesting_balance(&4), Some(transferred_amount));
		// and free balance.
		user_4_free_balance += transferred_amount;
		assert_eq!(Balances::free_balance(&4), user_4_free_balance);

		// Cannot insert a 4th vesting schedule when `MaxVestingSchedules` === 3
		assert_noop!(
			mock::Vesting::vested_transfer(Some(3).into(), 4, sched),
			Error::<Test>::AtMaxVestingSchedules,
		);
		// so the free balance does not change.
		assert_eq!(Balances::free_balance(&4), user_4_free_balance);

		// Account 4 has fully vested when all the schedules end
		System::set_block_number(ED + 10);
		assert_eq!(mock::Vesting::vesting_balance(&4), Some(0));
		// and after unlocking its schedules are removed from storage.
		assert_ok!(mock::Vesting::vest(Some(4).into()));
		assert!(!<Vesting<Test>>::contains_key(&4));
	});
}

#[test]
fn force_vested_transfer_works() {
	ExtBuilder::default()
		.existential_deposit(256)
		.build()
		.execute_with(|| {
			let user3_free_balance = Balances::free_balance(&3);
			let user4_free_balance = Balances::free_balance(&4);
			assert_eq!(user3_free_balance, 256 * 30);
			assert_eq!(user4_free_balance, 256 * 40);
			// Account 4 should not have any vesting yet.
			assert_eq!(mock::Vesting::vesting(&4), None);
			// Make the schedule for the new transfer.
			let new_vesting_schedule = VestingInfo::new::<Test>(
				256 * 5,
				64, // Vesting over 20 blocks
				10,
			);

			assert_noop!(
				mock::Vesting::force_vested_transfer(Some(4).into(), 3, 4, new_vesting_schedule),
				BadOrigin
			);
			assert_ok!(mock::Vesting::force_vested_transfer(
				RawOrigin::Root.into(),
				3,
				4,
				new_vesting_schedule
			));
			// Now account 4 should have vesting.
			assert_eq!(mock::Vesting::vesting(&4).unwrap()[0], new_vesting_schedule);
			assert_eq!(mock::Vesting::vesting(&4).unwrap().len(), 1);
			// Ensure the transfer happened correctly.
			let user3_free_balance_updated = Balances::free_balance(&3);
			assert_eq!(user3_free_balance_updated, 256 * 25);
			let user4_free_balance_updated = Balances::free_balance(&4);
			assert_eq!(user4_free_balance_updated, 256 * 45);
			// Account 4 has 5 * 256 locked.
			assert_eq!(mock::Vesting::vesting_balance(&4), Some(256 * 5));

			System::set_block_number(20);
			assert_eq!(System::block_number(), 20);

			// Account 4 has 5 * 64 units vested by block 20.
			assert_eq!(mock::Vesting::vesting_balance(&4), Some(10 * 64));

			System::set_block_number(30);
			assert_eq!(System::block_number(), 30);

			// Account 4 has fully vested.
			assert_eq!(mock::Vesting::vesting_balance(&4), Some(0));
			// and after unlocking its schedules are removed from storage.
			assert_ok!(mock::Vesting::vest(Some(4).into()));
			assert!(!<Vesting<Test>>::contains_key(&4));
		});
}

#[test]
fn force_vested_transfer_correctly_fails() {
	ExtBuilder::default()
		.existential_deposit(256)
		.build()
		.execute_with(|| {
			let user2_free_balance = Balances::free_balance(&2);
			let user4_free_balance = Balances::free_balance(&4);
			assert_eq!(user2_free_balance, 256 * 20);
			assert_eq!(user4_free_balance, 256 * 40);
			// Account 2 should already have a vesting schedule.
			let user2_vesting_schedule = VestingInfo::new::<Test>(
				256 * 20,
				256, // Vesting over 20 blocks
				10,
			);
			assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![user2_vesting_schedule]);

			// Too low transfer amount.
			let new_vesting_schedule_too_low =
				VestingInfo::new::<Test>(<Test as Config>::MinVestedTransfer::get() - 1, 64, 10);
			assert_noop!(
				mock::Vesting::force_vested_transfer(
					RawOrigin::Root.into(),
					3,
					4,
					new_vesting_schedule_too_low
				),
				Error::<Test>::AmountLow,
			);

			// `per_block` is 0.
			let schedule_per_block_0 = VestingInfo::new::<Test>(256, 0, 10);
			assert_noop!(
				mock::Vesting::force_vested_transfer(RawOrigin::Root.into(), 3, 4, schedule_per_block_0),
				Error::<Test>::InvalidScheduleParams,
			);

			// `locked` is 0.
			let schedule_locked_0 = VestingInfo::new::<Test>(0, 1, 10);
			assert_noop!(
				mock::Vesting::force_vested_transfer(RawOrigin::Root.into(), 3, 4, schedule_locked_0),
				Error::<Test>::AmountLow,
			);

			// Verify no currency transfer happened.
			assert_eq!(user2_free_balance, Balances::free_balance(&2));
			assert_eq!(user4_free_balance, Balances::free_balance(&4));
		});
}

#[test]
fn force_vested_transfer_allows_max_schedules() {
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		let mut user_4_free_balance = Balances::free_balance(&4);
		let max_schedules = <Test as Config>::MaxVestingSchedules::get();
		let sched = VestingInfo::new::<Test>(
			ED, 1, // Vest over 256 blocks.
			10,
		);
		// Add max amount schedules to user 4.
		for _ in 0 .. max_schedules {
			assert_ok!(mock::Vesting::force_vested_transfer(RawOrigin::Root.into(), 3, 4, sched));
		}
		// The schedules count towards vesting balance.
		let transferred_amount = ED * max_schedules as u64;
		assert_eq!(mock::Vesting::vesting_balance(&4), Some(transferred_amount));
		// and free balance.
		user_4_free_balance += transferred_amount;
		assert_eq!(Balances::free_balance(&4), user_4_free_balance);

		// Cannot insert a 4th vesting schedule when `MaxVestingSchedules` === 3
		assert_noop!(
			mock::Vesting::force_vested_transfer(RawOrigin::Root.into(), 3, 4, sched),
			Error::<Test>::AtMaxVestingSchedules,
		);
		// so the free balance does not change.
		assert_eq!(Balances::free_balance(&4), user_4_free_balance);

		// Account 4 has fully vested when all the schedules end
		System::set_block_number(ED + 10);
		assert_eq!(mock::Vesting::vesting_balance(&4), Some(0));
		// and after unlocking its schedules are removed from storage.
		assert_ok!(mock::Vesting::vest(Some(4).into()));
		assert!(!<Vesting<Test>>::contains_key(&4));
	});
}

#[test]
fn merge_schedules_that_have_not_started() {
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		// Account 2 should already have a vesting schedule.
		let sched0 = VestingInfo::new::<Test>(
			ED * 20,
			ED, // Vest over 20 blocks.
			10,
		);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0]);
		assert_eq!(Balances::usable_balance(&2), 0);

		// Add a schedule that is identical to the one that already exists
		mock::Vesting::vested_transfer(Some(3).into(), 2, sched0).unwrap();
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0, sched0]);
		assert_eq!(Balances::usable_balance(&2), 0);
		mock::Vesting::merge_schedules(Some(2).into(), 0, 1).unwrap();

		// Since we merged identical schedules, the new schedule finishes at the same
		// time as the original, just with double the amount
		let sched1 = VestingInfo::new::<Test>(
			sched0.locked() * 2,
			sched0.per_block() * 2,
			10, // starts at the block the schedules are merged
		);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched1]);

		assert_eq!(Balances::usable_balance(&2), 0);
	});
}

#[test]
fn merge_ongoing_schedules() {
	// Merging two schedules that have started will vest both before merging
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		// Account 2 should already have a vesting schedule.
		let sched0 = VestingInfo::new::<Test>(
			ED * 20,
			ED, // Vest over 20 blocks
			10,
		);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0]);

		let sched1 = VestingInfo::new::<Test>(
			256 * 10,
			256,                         // Vest over 10 blocks
			sched0.starting_block() + 5, // Start at block 15
		);
		mock::Vesting::vested_transfer(Some(4).into(), 2, sched1).unwrap();
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0, sched1]);

		// Got to half way through the second schedule where both schedules are actively vesting
		let cur_block = 20;
		System::set_block_number(cur_block);

		// user2 has no usable balances prior to the merge because they have not unlocked
		// with `vest` yet.
		assert_eq!(Balances::usable_balance(&2), 0);
		mock::Vesting::merge_schedules(Some(2).into(), 0, 1).unwrap();

		// Merging schedules vests all pre-existing schedules prior to merging, which is reflected
		// in user2's updated usable balance
		let sched0_vested_now = sched0.per_block() * (cur_block - sched0.starting_block());
		let sched1_vested_now = sched1.per_block() * (cur_block - sched1.starting_block());
		assert_eq!(Balances::usable_balance(&2), sched0_vested_now + sched1_vested_now);

		// The locked amount is the sum of what both schedules have locked at the current block.
		let sched2_locked = sched1
			.locked_at::<Identity>(cur_block)
			.saturating_add(sched0.locked_at::<Identity>(cur_block));
		// End block of the new schedule is the greater of either merged schedule
		let sched2_end = sched1.ending_block::<Identity>().max(sched0.ending_block::<Identity>());
		let sched2_duration = sched2_end - cur_block;
		// Based off the new schedules total locked and its duration, we can calculate the
		// amount to unlock per block.
		let sched2_per_block = sched2_locked / sched2_duration;

		let sched2 = VestingInfo::new::<Test>(sched2_locked, sched2_per_block, cur_block);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched2]);
	});
}

#[test]
fn merging_shifts_other_schedules_index() {
	// Schedules being merged are filtered out, schedules to the right of any merged
	// schedule shift left and the new, merged schedule is always last.
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		let sched0 = VestingInfo::new::<Test>(
			ED * 10,
			ED, // Vesting over 10 blocks
			10,
		);
		let sched1 = VestingInfo::new::<Test>(
			ED * 11,
			ED, // Vesting over 11 blocks
			11,
		);
		let sched2 = VestingInfo::new::<Test>(
			ED * 12,
			ED, // Vesting over 12 blocks
			12,
		);

		// Account 3 start out with no schedules
		assert_eq!(mock::Vesting::vesting(&3), None);
		// and some usable balance.
		let usable_balance = Balances::usable_balance(&3);
		assert_eq!(usable_balance, 30 * ED);

		let cur_block = 1;
		assert_eq!(System::block_number(), cur_block);

		// Transfer the above 3 schedules to account 3
		assert_ok!(mock::Vesting::vested_transfer(Some(4).into(), 3, sched0));
		assert_ok!(mock::Vesting::vested_transfer(Some(4).into(), 3, sched1));
		assert_ok!(mock::Vesting::vested_transfer(Some(4).into(), 3, sched2));

		// With no schedules vested or merged they are in the order they are created
		assert_eq!(mock::Vesting::vesting(&3).unwrap(), vec![sched0, sched1, sched2]);
		// and the usable balance has not changed.
		assert_eq!(usable_balance, Balances::usable_balance(&3));

		assert_ok!(mock::Vesting::merge_schedules(Some(3).into(), 0, 2));

		// Create the merged schedule of sched0 & sched2.
		// The merged schedule will have the max possible starting block,
		let sched3_start = sched1.starting_block().max(sched2.starting_block());
		// have locked equal to the sum of the two schedules locked through the current block,
		let sched3_locked =
			sched2.locked_at::<Identity>(cur_block) + sched0.locked_at::<Identity>(cur_block);
		// and will end at the max possible block.
		let sched3_end = sched2.ending_block::<Identity>().max(sched0.ending_block::<Identity>());
		let sched3_duration = sched3_end - sched3_start;
		let sched3_per_block = sched3_locked / sched3_duration;
		let sched3 = VestingInfo::new::<Test>(sched3_locked, sched3_per_block, sched3_start);

		// The not touched schedule moves left and the new merged schedule is appended.
		assert_eq!(mock::Vesting::vesting(&3).unwrap(), vec![sched1, sched3]);
		// The usable balance hasn't changed since none of the schedules have started.
		assert_eq!(Balances::usable_balance(&3), usable_balance);
	});
}

#[test]
fn merge_ongoing_and_yet_to_be_started_schedule() {
	// Merge an ongoing schedule that has had `vest` called and a schedule that has not already
	// started.
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		// Account 2 should already have a vesting schedule.
		let sched0 = VestingInfo::new::<Test>(
			ED * 20,
			ED, // Vesting over 20 blocks
			10,
		);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0]);

		// Fast forward to half way through the life of sched_1
		let mut cur_block = (sched0.starting_block() + sched0.ending_block::<Identity>()) / 2;
		assert_eq!(cur_block, 20);
		System::set_block_number(cur_block);

		// Prior to vesting there is no usable balance.
		let mut usable_balance = 0;
		assert_eq!(Balances::usable_balance(&2), usable_balance);
		// Vest the current schedules (which is just sched0 now).
		mock::Vesting::vest(Some(2).into()).unwrap();

		// After vesting the usable balance increases by the amount the unlocked amount.
		let sched0_vested_now = sched0.locked() - sched0.locked_at::<Identity>(cur_block);
		usable_balance += sched0_vested_now;
		assert_eq!(Balances::usable_balance(&2), usable_balance);

		// Go forward a block.
		cur_block += 1;
		System::set_block_number(cur_block);

		// And add a schedule that starts after this block, but before sched0 finishes.
		let sched1 = VestingInfo::new::<Test>(
			ED * 10,
			1, // Vesting over 256 * 10 (2560) blocks
			cur_block + 1,
		);
		mock::Vesting::vested_transfer(Some(4).into(), 2, sched1).unwrap();

		// Merge the schedules before sched1 starts.
		mock::Vesting::merge_schedules(Some(2).into(), 0, 1).unwrap();
		// After merging, the usable balance only changes by the amount sched0 vested since we
		// last called `vest` (which is just 1 block). The usable balance is not affected by
		// sched1 because it has not started yet.
		usable_balance += sched0.per_block();
		assert_eq!(Balances::usable_balance(&2), usable_balance);

		// The resulting schedule will have the later starting block of the two,
		let sched2_start = sched1.starting_block();
		// have locked equal to the sum of the two schedules locked through the current block,
		let sched2_locked =
			sched0.locked_at::<Identity>(cur_block) + sched1.locked_at::<Identity>(cur_block);
		// and will end at the max possible block.
		let sched2_end = sched0.ending_block::<Identity>().max(sched1.ending_block::<Identity>());
		let sched2_duration = sched2_end - sched2_start;
		let sched2_per_block = sched2_locked / sched2_duration;

		let sched2 = VestingInfo::new::<Test>(sched2_locked, sched2_per_block, sched2_start);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched2]);
	});
}

#[test]
fn merge_finishing_and_ongoing_schedule() {
	// If a schedule finishes by the current block we treat the ongoing schedule,
	// without any alterations, as the merged one.
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		// Account 2 should already have a vesting schedule.
		let sched0 = VestingInfo::new::<Test>(
			ED * 20,
			ED, // Vesting over 20 blocks.
			10,
		);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0]);

		let sched1 = VestingInfo::new::<Test>(
			ED * 40,
			ED, // Vesting over 40 blocks.
			10,
		);
		assert_ok!(mock::Vesting::vested_transfer(Some(4).into(), 2, sched1));

		// Transfer a 3rd schedule, so we can demonstrate how schedule indices change.
		// (We are not merging this schedule.)
		let sched2 = VestingInfo::new::<Test>(
			ED * 30,
			ED, // Vesting over 30 blocks.
			10,
		);
		assert_ok!(mock::Vesting::vested_transfer(Some(3).into(), 2, sched2));

		// The schedules are in expected order prior to merging.
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0, sched1, sched2]);

		// Fast forward to sched0's end block.
		let cur_block = sched0.ending_block::<Identity>();
		System::set_block_number(cur_block);
		assert_eq!(System::block_number(), 30);

		// Prior to merge_schedules and with no vest/vest_other called the user has no usable
		// balance.
		assert_eq!(Balances::usable_balance(&2), 0);
		assert_ok!(mock::Vesting::merge_schedules(Some(2).into(), 0, 1));

		// sched2 is now the first, since sched0 & sched1 get filtered out while "merging".
		// sched1 gets treated like the new merged schedule by getting pushed onto back
		// of the vesting schedules vec. Note: sched0 finished at the current block.
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched2, sched1]);

		// sched0 has finished, so its funds are fully unlocked.
		let sched0_unlocked_now = sched0.locked();
		// The remaining schedules are ongoing, so their funds are partially unlocked.
		let sched1_unlocked_now = sched1.locked() - sched1.locked_at::<Identity>(cur_block);
		let sched2_unlocked_now = sched2.locked() - sched2.locked_at::<Identity>(cur_block);

		// Since merging also vests all the schedules, the users usable balance after merging
		// includes all pre-existing schedules unlocked through the current block, including
		// schedules not merged.
		assert_eq!(
			Balances::usable_balance(&2),
			sched0_unlocked_now + sched1_unlocked_now + sched2_unlocked_now
		);
	});
}

#[test]
fn merge_finishing_schedules_does_not_create_a_new_one() {
	// If both schedules finish by the current block we don't create new one
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		// Account 2 should already have a vesting schedule.
		let sched0 = VestingInfo::new::<Test>(
			ED * 20,
			ED, // 20 block duration.
			10,
		);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0]);

		// Create sched1 and transfer it to account 2.
		let sched1 = VestingInfo::new::<Test>(
			ED * 30,
			ED, // 30 block duration.
			10,
		);
		assert_ok!(mock::Vesting::vested_transfer(Some(3).into(), 2, sched1));
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0, sched1]);

		let all_scheds_end =
			sched0.ending_block::<Identity>().max(sched1.ending_block::<Identity>());
		assert_eq!(all_scheds_end, 40);
		System::set_block_number(all_scheds_end);

		// Prior to merge_schedules and with no vest/vest_other called the user has no usable
		// balance.
		assert_eq!(Balances::usable_balance(&2), 0);

		// Merge schedule 0 and 1.
		mock::Vesting::merge_schedules(Some(2).into(), 0, 1).unwrap();
		// The user no longer has any more vesting schedules because they both ended at the
		// block they where merged.
		assert!(!<Vesting<Test>>::contains_key(&2));
		// And their usable balance has increased by the total amount locked in the merged
		// schedules.
		assert_eq!(Balances::usable_balance(&2), sched0.locked() + sched1.locked());
	});
}

#[test]
fn merge_schedules_throws_proper_errors() {
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		// Account 2 should already have a vesting schedule.
		let sched0 = VestingInfo::new::<Test>(
			ED * 20,
			ED, // 20 block duration.
			10,
		);
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0]);

		// There is only 1 vesting schedule.
		assert_noop!(
			mock::Vesting::merge_schedules(Some(2).into(), 0, 1),
			Error::<Test>::ScheduleIndexOutOfBounds
		);

		// There are 0 vesting schedules.
		assert_eq!(mock::Vesting::vesting(&4), None);
		assert_noop!(mock::Vesting::merge_schedules(Some(4).into(), 0, 1), Error::<Test>::NotVesting);

		// There are enough schedules to merge but an index is non-existent.
		mock::Vesting::vested_transfer(Some(3).into(), 2, sched0).unwrap();
		assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![sched0, sched0]);
		assert_noop!(
			mock::Vesting::merge_schedules(Some(2).into(), 0, 2),
			Error::<Test>::ScheduleIndexOutOfBounds
		);

		// It is a storage noop with no errors if the indexes are the same.
		assert_storage_noop!(mock::Vesting::merge_schedules(Some(2).into(), 0, 0).unwrap());
	});
}

#[test]
fn generates_multiple_schedules_from_genesis_config() {
	let vesting_config = vec![
		// 5 * existential deposit locked
		(1, 0, 10, 5 * ED),
		// 1 * existential deposit locked
		(2, 10, 20, 19 * ED),
		// 2 * existential deposit locked
		(2, 10, 20, 18 * ED),
		// 1 * existential deposit locked
		(12, 10, 20, 9 * ED),
		// 2 * existential deposit locked
		(12, 10, 20, 8 * ED),
		// 3 * existential deposit locked
		(12, 10, 20, 7 * ED),
	];
	ExtBuilder::default()
		.existential_deposit(ED)
		.vesting_genesis_config(vesting_config)
		.build()
		.execute_with(|| {
			let user1_sched1 = VestingInfo::new::<Test>(5 * ED, 128, 0u64);
			assert_eq!(mock::Vesting::vesting(&1).unwrap(), vec![user1_sched1]);

			let user2_sched1 = VestingInfo::new::<Test>(1 * ED, 12, 10u64);
			let user2_sched2 = VestingInfo::new::<Test>(2 * ED, 25, 10u64);
			assert_eq!(mock::Vesting::vesting(&2).unwrap(), vec![user2_sched1, user2_sched2]);

			let user12_sched1 = VestingInfo::new::<Test>(1 * ED, 12, 10u64);
			let user12_sched2 = VestingInfo::new::<Test>(2 * ED, 25, 10u64);
			let user12_sched3 = VestingInfo::new::<Test>(3 * ED, 38, 10u64);
			assert_eq!(
				mock::Vesting::vesting(&12).unwrap(),
				vec![user12_sched1, user12_sched2, user12_sched3]
			);
		});
}

#[test]
#[should_panic]
fn multiple_schedules_from_genesis_config_errors() {
	// MaxVestingSchedules is 3, but this config has 4 for account 12 so we panic when building
	// from genesis.
	let vesting_config =
		vec![(12, 10, 20, ED), (12, 10, 20, ED), (12, 10, 20, ED), (12, 10, 20, ED)];
	ExtBuilder::default()
		.existential_deposit(ED)
		.vesting_genesis_config(vesting_config)
		.build();
}

#[test]
fn merge_vesting_info_handles_per_block_0() {
	// Faulty schedules with an infinite duration (per_block == 0) can be merged to create
	// a schedule that vest 1 per_block, (helpful for faulty, legacy schedules).
	ExtBuilder::default().existential_deposit(ED).build().execute_with(|| {
		let sched0 = VestingInfo::new::<Test>(
			ED, 0, // Vesting over infinite blocks.
			1,
		);
		let sched1 = VestingInfo::new::<Test>(
			ED * 2,
			0, // Vesting over infinite blocks.
			10,
		);

		let cur_block = 5;
		let expected_locked =
			sched0.locked_at::<Identity>(cur_block) + sched1.locked_at::<Identity>(cur_block);
		let expected_sched = VestingInfo::new::<Test>(
			expected_locked,
			1, // per_block of 1, which corrects existing schedules.
			10,
		);
		assert_eq!(mock::Vesting::merge_vesting_info(cur_block, sched0, sched1), Some(expected_sched),);
	});
}

#[test]
fn vesting_info_validation_works() {
	let min_transfer = <Test as Config>::MinVestedTransfer::get();
	// Does not check for min transfer.
	match VestingInfo::new::<Test>(min_transfer - 1, 1u64, 10u64).validate::<Test>() {
		Ok(_) => (),
		_ => panic!(),
	}

	// `locked` cannot be 0.
	match VestingInfo::new::<Test>(0, 1u64, 10u64).validate::<Test>() {
		Err(Error::<Test>::InvalidScheduleParams) => (),
		_ => panic!(),
	}

	// `per_block` cannot be 0.
	match VestingInfo::new::<Test>(min_transfer + 1, 0u64, 10u64).validate() {
		Err(Error::<Test>::InvalidScheduleParams) => (),
		_ => panic!(),
	}

	// With valid inputs it does not error and does not modify the inputs.
	assert_eq!(
		VestingInfo::new::<Test>(min_transfer, 1u64, 10u64).validate::<Test>().unwrap(),
		VestingInfo::new::<Test>(min_transfer, 1u64, 10u64)
	);

	// `per_block` is never bigger than `locked`.
	assert_eq!(
		VestingInfo::new::<Test>(256u64, 256 * 2u64, 10u64).validate::<Test>().unwrap(),
		VestingInfo::new::<Test>(256u64, 256u64, 10u64)
	);
}

#[test]
fn vesting_info_ending_block_works() {
	// Treats `per_block` 0 as a `per_block` of 1 to accommodate faulty, legacy schedules.
	let per_block_0 = VestingInfo::new::<Test>(256u32, 0u32, 10u32);
	assert_eq!(
		per_block_0.ending_block::<Identity>(),
		per_block_0.locked() + per_block_0.starting_block()
	);
	let per_block_1 = VestingInfo::new::<Test>(256u32, 1u32, 10u32);
	assert_eq!(per_block_0.ending_block::<Identity>(), per_block_1.ending_block::<Identity>());

	// `per_block >= locked` always results in a schedule ending the block after it starts
	let per_block_gt_locked = VestingInfo::new::<Test>(256u32, 256 * 2u32, 10u32);
	assert_eq!(
		per_block_gt_locked.ending_block::<Identity>(),
		1 + per_block_gt_locked.starting_block()
	);
	let per_block_eq_locked = VestingInfo::new::<Test>(256u32, 256u32, 10u32);
	assert_eq!(
		per_block_gt_locked.ending_block::<Identity>(),
		per_block_eq_locked.ending_block::<Identity>()
	);

	// Correctly calcs end if `locked % per_block != 0`. (We need a block to unlock the remainder).
	let imperfect_per_block = VestingInfo::new::<Test>(256u32, 250u32, 10u32);
	assert_eq!(
		imperfect_per_block.ending_block::<Identity>(),
		imperfect_per_block.starting_block() + 2u32,
	);
	assert_eq!(
		imperfect_per_block.locked_at::<Identity>(imperfect_per_block.ending_block::<Identity>()),
		0
	);
}
