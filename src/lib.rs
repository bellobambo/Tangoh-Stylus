#![cfg_attr(not(feature = "export-abi"), no_main)]
extern crate alloc;

use stylus_sdk::{
    prelude::*,
    storage::{StorageAddress, StorageB256, StorageI256, StorageU8, StorageU256},
};
use alloy_primitives::{Address, U256, I256, U8, B256};
use alloy_sol_types::{sol, SolError};

sol! {
    event TicketCreated(uint256 indexed ticket_id, address indexed creator);
    event FundraisingStarted(uint256 indexed ticket_id, address approver); 
    event FundsReceived(uint256 indexed ticket_id, address contributor);
    event FundraisingClosed(uint256 indexed ticket_id, uint256 total_raised);
    event StatusChanged(uint256 indexed ticket_id, uint8 new_status);
    event UserRegistered(address indexed user); 
    event FundsWithdrawn(uint256 indexed ticket_id, address indexed recipient, uint256 amount);

    error NotAuthorized();
    error InvalidStatus();
    error TimeWindowInvalid();
    error TicketNotFound();
    error NoFundsToWithdraw();
}

const STATUS_PENDING: u8 = 0;
const STATUS_FUNDRAISING: u8 = 1;
const STATUS_PROJECT_PENDING: u8 = 2;
const STATUS_COMPLETED: u8 = 3;

const ROLE_EXCO: u8 = 1;

#[storage]
pub struct User {
    name: StorageB256, 
    role: StorageU8, 
}

#[storage]
pub struct Ticket {
    creator: StorageAddress,
    approver: StorageAddress, 
    title: StorageB256,       // NEW: Added Title
    description: StorageB256, 
    votes: StorageI256,
    target_amount: StorageU256,
    raised_amount: StorageU256,
    start_time: StorageU256,
    end_time: StorageU256,
    status: StorageU8,
}

sol_storage! {
    #[entrypoint]
    pub struct CollegeFundraiser {
        address escrow_account;
        address owner;
        uint256 ticket_count;
        
        mapping(address => User) users;
        mapping(uint256 => Ticket) tickets;
    }
}

#[public]
impl CollegeFundraiser {
    
    pub fn init(&mut self, escrow: Address) -> Result<(), Vec<u8>> {
        self.escrow_account.set(escrow);
        let sender = self.vm().msg_sender();
        self.owner.set(sender);
        
        let mut user = self.users.setter(sender);
        user.role.set(U8::from(ROLE_EXCO));
        Ok(())
    }

    pub fn register_user(&mut self, name: B256, role: u8) -> Result<(), Vec<u8>> {
        let sender = self.vm().msg_sender();
        let mut user = self.users.setter(sender);
        
        user.name.set(name);
        user.role.set(U8::from(role));
        
        log(self.vm(), UserRegistered { user: sender });
        Ok(())
    }

    // UPDATED: Added title to arguments
    pub fn create_ticket(&mut self, title: B256, description: B256) -> Result<U256, Vec<u8>> {
        let ticket_id = self.ticket_count.get();
        let creator = self.vm().msg_sender();
        
        let mut ticket = self.tickets.setter(ticket_id);
        ticket.creator.set(creator);
        ticket.title.set(title); // Set the title
        ticket.description.set(description);
        ticket.status.set(U8::from(STATUS_PENDING));
        ticket.votes.set(I256::ZERO);

        self.ticket_count.set(ticket_id + U256::from(1));
        
        log(self.vm(), TicketCreated { ticket_id, creator });
        Ok(ticket_id)
    }

    pub fn vote(&mut self, ticket_id: U256, upvote: bool) -> Result<(), Vec<u8>> {
        if ticket_id >= self.ticket_count.get() {
            return Err(TicketNotFound{}.abi_encode());
        }

        let mut ticket = self.tickets.setter(ticket_id);
        if ticket.status.get() != U8::from(STATUS_PENDING) {
            return Err(InvalidStatus{}.abi_encode());
        }

        let val = if upvote { I256::try_from(1).unwrap() } else { I256::try_from(-1).unwrap() };
        
        let current_votes = ticket.votes.get();
        ticket.votes.set(current_votes + val);
        Ok(())
    }

    pub fn approve_ticket(&mut self, ticket_id: U256, target_amount: U256, start_time: U256, end_time: U256) -> Result<(), Vec<u8>> {
        let sender = self.vm().msg_sender();
        if self.users.getter(sender).role.get().to::<u8>() != ROLE_EXCO {
             return Err(NotAuthorized{}.abi_encode());
        }

        let mut ticket = self.tickets.setter(ticket_id);

        if ticket.status.get() != U8::from(STATUS_PENDING) {
             return Err(InvalidStatus{}.abi_encode());
        }

        ticket.approver.set(sender);
        ticket.target_amount.set(target_amount);
        ticket.start_time.set(start_time);
        ticket.end_time.set(end_time);
        ticket.status.set(U8::from(STATUS_FUNDRAISING));

        log(self.vm(), FundraisingStarted { ticket_id, approver: sender });
        Ok(())
    }

    pub fn close_fundraising(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        let sender = self.vm().msg_sender();
        let mut ticket = self.tickets.setter(ticket_id);

        if self.users.getter(sender).role.get().to::<u8>() != ROLE_EXCO {
             return Err(NotAuthorized{}.abi_encode());
        }
        
        if ticket.status.get() != U8::from(STATUS_FUNDRAISING) {
            return Err(InvalidStatus{}.abi_encode());
        }

        let amount_raised = ticket.raised_amount.get();

        ticket.status.set(U8::from(STATUS_PROJECT_PENDING));
        
        log(self.vm(), FundraisingClosed { ticket_id, total_raised: amount_raised });
        Ok(())
    }

    pub fn withdraw_funds(&mut self, ticket_id: U256, recipient: Address) -> Result<(), Vec<u8>> {
        let sender = self.vm().msg_sender();
        let mut ticket = self.tickets.setter(ticket_id);

        if sender != ticket.approver.get() {
             return Err(NotAuthorized{}.abi_encode());
        }

        let status = ticket.status.get().to::<u8>();

        if status != STATUS_PROJECT_PENDING && status != STATUS_COMPLETED {
            return Err(InvalidStatus{}.abi_encode());
        }

        let amount = ticket.raised_amount.get();
        if amount == U256::ZERO {
            return Err(NoFundsToWithdraw{}.abi_encode());
        }

        ticket.raised_amount.set(U256::ZERO);
        let _ = self.vm().transfer_eth(recipient, amount);

        log(self.vm(), FundsWithdrawn { ticket_id, recipient, amount });
        Ok(())
    }

    pub fn mark_project_complete(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        let sender = self.vm().msg_sender();
        if self.users.getter(sender).role.get().to::<u8>() != ROLE_EXCO {
             return Err(NotAuthorized{}.abi_encode());
        }

        let mut ticket = self.tickets.setter(ticket_id);

        if ticket.status.get() != U8::from(STATUS_PROJECT_PENDING) {
            return Err(InvalidStatus{}.abi_encode());
        }

        ticket.status.set(U8::from(STATUS_COMPLETED));
        log(self.vm(), StatusChanged { ticket_id, new_status: STATUS_COMPLETED });
        Ok(())
    }

    #[payable]
    pub fn fund_ticket(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        let sender = self.vm().msg_sender();
        let msg_value = self.vm().msg_value();
        let block_timestamp = U256::from(self.vm().block_timestamp());

        let mut ticket = self.tickets.setter(ticket_id);

        if ticket.status.get() != U8::from(STATUS_FUNDRAISING) {
            return Err(InvalidStatus{}.abi_encode());
        }
        
        if block_timestamp < ticket.start_time.get() || block_timestamp > ticket.end_time.get() {
            return Err(TimeWindowInvalid{}.abi_encode());
        }

        let current_raised = ticket.raised_amount.get();
        ticket.raised_amount.set(current_raised + msg_value);

        log(self.vm(), FundsReceived { ticket_id, contributor: sender });
        Ok(())
    }

    // --- View Functions ---

    // UPDATED: Now returns Title as well
    pub fn get_ticket(&self, ticket_id: U256) -> Result<(Address, Address, B256, B256, I256, U256, U256, u8), Vec<u8>> {
        let ticket = self.tickets.getter(ticket_id);
        Ok((
            ticket.creator.get(),
            ticket.approver.get(), 
            ticket.title.get(),       // Return Title
            ticket.description.get(), 
            ticket.votes.get(),
            ticket.target_amount.get(),
            ticket.raised_amount.get(),
            ticket.status.get().to(),
        ))
    }

    pub fn get_user(&self, user_address: Address) -> Result<(B256, u8), Vec<u8>> {
        let user = self.users.getter(user_address);
        Ok((
            user.name.get(), 
            user.role.get().to::<u8>(),
        ))
    }

    pub fn get_ticket_count(&self) -> U256 {
        self.ticket_count.get()
    }

    pub fn get_escrow_account(&self) -> Address {
        self.escrow_account.get()
    }

    pub fn get_owner(&self) -> Address {
        self.owner.get()
    }
}

// Add this at the bottom of src/lib.rs

#[cfg(test)]
mod tests {
    use super::*;
    use stylus_sdk::testing::*;
    use alloy_primitives::{address, uint};

    // --- Helper: Setup Contract ---
    fn setup() -> (TestVM, CollegeFundraiser) {
        let vm = TestVM::default();
        let mut contract = CollegeFundraiser::from(&vm);
        
        // Mock Addresses
        let escrow = address!("0000000000000000000000000000000000000099");
        
        // Initialize contract (Msg.sender is default 0x...0)
        // We act as the deployer here
        contract.init(escrow).unwrap();

        (vm, contract)
    }

    // --- Helper: Mock Data ---
    fn mock_title() -> B256 {
        // "Solar Panel Project" in bytes32
        "536f6c61722050616e656c2050726f6a65637400000000000000000000000000"
            .parse()
            .unwrap()
    }

    fn mock_desc() -> B256 {
        // "Description" in bytes32
        "4465736372697074696f6e000000000000000000000000000000000000000000"
            .parse()
            .unwrap()
    }

    #[test]
    fn test_initialization() {
        let (_vm, contract) = setup();
        
        // Check Owner
        assert_eq!(contract.owner.get(), Address::ZERO); // Default TestVM sender is 0x0...0
        
        // Check Ticket Count
        assert_eq!(contract.ticket_count.get(), U256::ZERO);

        // Check Deployer is Exco (Role 1)
        let user = contract.users.getter(Address::ZERO);
        assert_eq!(user.role.get().to::<u8>(), ROLE_EXCO);
    }

    #[test]
    fn test_user_registration() {
        let (mut vm, mut contract) = setup();
        let alice = address!("0000000000000000000000000000000000000001");
        
        // Switch to Alice
        vm.set_msg_sender(alice);

        let name_hash: B256 = "416c696365000000000000000000000000000000000000000000000000000000".parse().unwrap(); // "Alice"
        
        // Alice registers as Student (0)
        contract.register_user(name_hash, 0).unwrap();

        // Verify
        let (stored_name, stored_role) = contract.get_user(alice).unwrap();
        assert_eq!(stored_name, name_hash);
        assert_eq!(stored_role, 0);
    }

    #[test]
    fn test_create_ticket_and_vote() {
        let (mut vm, mut contract) = setup();
        let student = address!("0000000000000000000000000000000000000002");
        
        vm.set_msg_sender(student);

        // 1. Create Ticket
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();
        assert_eq!(ticket_id, U256::ZERO);

        // 2. Vote Up
        contract.vote(ticket_id, true).unwrap();
        
        // Verify Vote count = 1
        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.votes.get(), I256::try_from(1).unwrap());
    }

    #[test]
    fn test_approve_workflow_permissions() {
        let (mut vm, mut contract) = setup();
        let student = address!("0000000000000000000000000000000000000002");
        let exco = address!("0000000000000000000000000000000000000003");

        // Register Users
        vm.set_msg_sender(student);
        contract.register_user(B256::ZERO, 0).unwrap();
        
        vm.set_msg_sender(exco);
        contract.register_user(B256::ZERO, 1).unwrap();

        // Student creates ticket
        vm.set_msg_sender(student);
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();

        // Student tries to Approve (Should Fail)
        let target = uint!(1000_U256);
        let start = U256::ZERO;
        let end = uint!(9999999999_U256);
        
        let res = contract.approve_ticket(ticket_id, target, start, end);
        assert!(res.is_err()); // NotAuthorized

        // Exco tries to Approve (Should Succeed)
        vm.set_msg_sender(exco);
        let res = contract.approve_ticket(ticket_id, target, start, end);
        assert!(res.is_ok());

        // Verify Status moved to FUNDRAISING (1)
        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.status.get().to::<u8>(), STATUS_FUNDRAISING);
        
        // Verify Approver was saved
        assert_eq!(ticket.approver.get(), exco);
    }

    #[test]
    fn test_funding_logic() {
        let (mut vm, mut contract) = setup();
        let exco = Address::ZERO; // Deployer is Exco

        // Create and Approve Ticket
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();
        
        // Set time window
        let start_time = uint!(100_U256);
        let end_time = uint!(200_U256);
        
        contract.approve_ticket(ticket_id, uint!(1000_U256), start_time, end_time).unwrap();

        // Simulate Time Passing (Inside Window)
        vm.set_block_timestamp(150); 
        vm.set_msg_value(uint!(500_U256)); // Sending 500 wei

        // Fund
        contract.fund_ticket(ticket_id).unwrap();

        // Verify Balance
        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.raised_amount.get(), uint!(500_U256));
    }

    #[test]
    fn test_withdrawal_security() {
        let (mut vm, mut contract) = setup();
        
        // Actors
        let exco_1 = address!("000000000000000000000000000000000000000A");
        let exco_2 = address!("000000000000000000000000000000000000000B");
        let recipient = address!("0000000000000000000000000000000000000099");

        // Register Excos
        vm.set_msg_sender(exco_1);
        contract.register_user(B256::ZERO, 1).unwrap();
        vm.set_msg_sender(exco_2);
        contract.register_user(B256::ZERO, 1).unwrap();

        // 1. Create Ticket
        vm.set_msg_sender(exco_1);
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();

        // 2. EXCO_1 Approves it
        let end_time = uint!(9999999999_U256);
        contract.approve_ticket(ticket_id, uint!(1000_U256), U256::ZERO, end_time).unwrap();

        // 3. Fund it
        vm.set_msg_value(uint!(500_U256));
        contract.fund_ticket(ticket_id).unwrap();

        // 4. Close Fundraising
        contract.close_fundraising(ticket_id).unwrap();

        // 5. EXCO_2 tries to withdraw (Should Fail - Wrong Approver)
        vm.set_msg_sender(exco_2);
        let res = contract.withdraw_funds(ticket_id, recipient);
        assert!(res.is_err()); // NotAuthorized

        // 6. EXCO_1 tries to withdraw (Should Succeed)
        vm.set_msg_sender(exco_1);
        let res = contract.withdraw_funds(ticket_id, recipient);
        assert!(res.is_ok());

        // 7. Verify Balance is now 0 on ticket
        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.raised_amount.get(), U256::ZERO);
    }
}