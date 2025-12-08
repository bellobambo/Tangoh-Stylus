#![cfg_attr(not(any(feature = "export-abi", test)), no_main)]
extern crate alloc;

use stylus_sdk::{
    prelude::*,
    storage::{StorageAddress, StorageB256, StorageI256, StorageU8, StorageU256, StorageBool, StorageMap},
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
    event TicketAcknowledged(uint256 indexed ticket_id, address indexed creator);

    error NotAuthorized();
    error InvalidStatus();
    error TimeWindowInvalid();
    error TicketNotFound();
    error NoFundsToWithdraw();
    error AlreadyVoted();
    error AlreadyAcknowledged();
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
    title: StorageB256,
    description: StorageB256, 
    votes: StorageI256,
    target_amount: StorageU256,
    raised_amount: StorageU256,
    start_time: StorageU256,
    end_time: StorageU256,
    status: StorageU8,
    acknowledged: StorageBool,
    voter_map: StorageMap<Address, StorageBool>,
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

    pub fn create_ticket(&mut self, title: B256, description: B256) -> Result<U256, Vec<u8>> {
        let ticket_id = self.ticket_count.get();
        let creator = self.vm().msg_sender();
        
        let mut ticket = self.tickets.setter(ticket_id);
        ticket.creator.set(creator);
        ticket.title.set(title);
        ticket.description.set(description);
        ticket.status.set(U8::from(STATUS_PENDING));
        ticket.votes.set(I256::ZERO);
        ticket.acknowledged.set(false);

        self.ticket_count.set(ticket_id + U256::from(1));
        
        log(self.vm(), TicketCreated { ticket_id, creator });
        Ok(ticket_id)
    }

    pub fn vote(&mut self, ticket_id: U256, upvote: bool) -> Result<(), Vec<u8>> {
        if ticket_id >= self.ticket_count.get() {
            return Err(TicketNotFound{}.abi_encode());
        }

        let sender = self.vm().msg_sender();
        let mut ticket = self.tickets.setter(ticket_id);
        
        if ticket.status.get() != U8::from(STATUS_PENDING) {
            return Err(InvalidStatus{}.abi_encode());
        }

        if ticket.voter_map.get(sender) {
            return Err(AlreadyVoted{}.abi_encode());
        }

        ticket.voter_map.insert(sender, true);

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

    pub fn acknowledge_ticket(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        if ticket_id >= self.ticket_count.get() {
            return Err(TicketNotFound{}.abi_encode());
        }

        let sender = self.vm().msg_sender();
        let mut ticket = self.tickets.setter(ticket_id);

        if sender != ticket.creator.get() {
            return Err(NotAuthorized{}.abi_encode());
        }

        if ticket.status.get() != U8::from(STATUS_COMPLETED) {
            return Err(InvalidStatus{}.abi_encode());
        }

        if ticket.acknowledged.get() {
            return Err(AlreadyAcknowledged{}.abi_encode());
        }

        ticket.acknowledged.set(true);
        log(self.vm(), TicketAcknowledged { ticket_id, creator: sender });
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

    pub fn get_ticket(&self, ticket_id: U256) -> Result<(Address, Address, B256, B256, I256, U256, U256, u8, bool), Vec<u8>> {
        let ticket = self.tickets.getter(ticket_id);
        Ok((
            ticket.creator.get(),
            ticket.approver.get(), 
            ticket.title.get(),
            ticket.description.get(), 
            ticket.votes.get(),
            ticket.target_amount.get(),
            ticket.raised_amount.get(),
            ticket.status.get().to(),
            ticket.acknowledged.get(),
        ))
    }

    pub fn has_voted(&self, ticket_id: U256, user: Address) -> Result<bool, Vec<u8>> {
        if ticket_id >= self.ticket_count.get() {
            return Err(TicketNotFound{}.abi_encode());
        }
        
        let ticket = self.tickets.getter(ticket_id);
        Ok(ticket.voter_map.get(user))
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

#[cfg(test)]
mod tests {
    use super::*;
    use stylus_sdk::testing::*;
    use alloy_primitives::{address, uint};

    fn setup() -> (TestVM, CollegeFundraiser) {
        let vm = TestVM::default();
        let mut contract = CollegeFundraiser::from(&vm);
        
        let escrow = address!("0000000000000000000000000000000000000099");
        contract.init(escrow).unwrap();

        (vm, contract)
    }

    fn mock_title() -> B256 {
        "536f6c61722050616e656c2050726f6a65637400000000000000000000000000"
            .parse()
            .unwrap()
    }

    fn mock_desc() -> B256 {
        "4465736372697074696f6e000000000000000000000000000000000000000000"
            .parse()
            .unwrap()
    }

    #[test]
    fn test_initialization() {
        let (_vm, contract) = setup();
        
        assert_eq!(contract.ticket_count.get(), U256::ZERO);

        let user = contract.users.getter(contract.vm().msg_sender());
        assert_eq!(user.role.get().to::<u8>(), ROLE_EXCO);
    }

    #[test]
    fn test_user_registration() {
        let (_vm, mut contract) = setup();

        let name_hash: B256 = "416c696365000000000000000000000000000000000000000000000000000000".parse().unwrap();
        
        contract.register_user(name_hash, 0).unwrap();

        let sender = contract.vm().msg_sender();
        let (stored_name, stored_role) = contract.get_user(sender).unwrap();
        assert_eq!(stored_name, name_hash);
        assert_eq!(stored_role, 0);
    }

    #[test]
    fn test_duplicate_vote_prevention() {
        let (_vm, mut contract) = setup();
        
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();
        
        let sender = contract.vm().msg_sender();
        
        // First vote should succeed
        contract.vote(ticket_id, true).unwrap();
        
        // Check has_voted returns true
        assert!(contract.has_voted(ticket_id, sender).unwrap());
        
        // Second vote should fail
        let res = contract.vote(ticket_id, true);
        assert!(res.is_err());
    }

    #[test]
    fn test_acknowledgment_workflow() {
        let (_vm, mut contract) = setup();
        
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();

        // Approve (deployer is exco)
        contract.approve_ticket(ticket_id, uint!(1000_U256), U256::ZERO, uint!(9999999999_U256)).unwrap();

        // Fund ticket
        contract.fund_ticket(ticket_id).unwrap();

        // Close fundraising
        contract.close_fundraising(ticket_id).unwrap();

        // Mark complete
        contract.mark_project_complete(ticket_id).unwrap();

        // Acknowledge
        contract.acknowledge_ticket(ticket_id).unwrap();

        // Verify acknowledged status
        let (_, _, _, _, _, _, _, _, acknowledged) = contract.get_ticket(ticket_id).unwrap();
        assert!(acknowledged);

        // Try to acknowledge again (should fail)
        let res = contract.acknowledge_ticket(ticket_id);
        assert!(res.is_err());
    }

    #[test]
    fn test_approve_workflow() {
        let (_vm, mut contract) = setup();

        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();

        let target = uint!(1000_U256);
        let start = U256::ZERO;
        let end = uint!(9999999999_U256);
        
        // Deployer (EXCO) approves
        let res = contract.approve_ticket(ticket_id, target, start, end);
        assert!(res.is_ok());

        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.status.get().to::<u8>(), STATUS_FUNDRAISING);
    }

    #[test]
    fn test_funding_logic() {
        let (_vm, mut contract) = setup();

        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();
        
        let start_time = U256::ZERO;
        let end_time = uint!(9999999999_U256);
        
        contract.approve_ticket(ticket_id, uint!(1000_U256), start_time, end_time).unwrap();

        // Note: Testing with msg_value may require different approach in stylus-sdk
        contract.fund_ticket(ticket_id).unwrap();

        let ticket = contract.tickets.getter(ticket_id);
        // Verify status is still fundraising
        assert_eq!(ticket.status.get().to::<u8>(), STATUS_FUNDRAISING);
    }

    #[test]
    fn test_withdrawal() {
        let (_vm, mut contract) = setup();
        
        let recipient = address!("0000000000000000000000000000000000000099");

        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();

        let end_time = uint!(9999999999_U256);
        contract.approve_ticket(ticket_id, uint!(1000_U256), U256::ZERO, end_time).unwrap();

        // Manually set raised_amount since we can't set msg_value in tests
        let mut ticket = contract.tickets.setter(ticket_id);
        ticket.raised_amount.set(uint!(500_U256));

        contract.close_fundraising(ticket_id).unwrap();

        // Deployer (approver) withdraws
        let res = contract.withdraw_funds(ticket_id, recipient);
        assert!(res.is_ok());

        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.raised_amount.get(), U256::ZERO);
    }

    #[test]
    fn test_ticket_creation() {
        let (_vm, mut contract) = setup();
        
        let title = mock_title();
        let desc = mock_desc();
        
        let ticket_id = contract.create_ticket(title, desc).unwrap();
        
        assert_eq!(ticket_id, U256::ZERO);
        assert_eq!(contract.get_ticket_count(), U256::from(1));
        
        let (creator, _approver, stored_title, stored_desc, votes, _target, _raised, status, acknowledged) = 
            contract.get_ticket(ticket_id).unwrap();
        
        assert_eq!(creator, contract.vm().msg_sender());
        assert_eq!(stored_title, title);
        assert_eq!(stored_desc, desc);
        assert_eq!(votes, I256::ZERO);
        assert_eq!(status, STATUS_PENDING);
        assert_eq!(acknowledged, false);
    }

    #[test]
    fn test_voting() {
        let (_vm, mut contract) = setup();
        
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();
        
        // Upvote
        contract.vote(ticket_id, true).unwrap();
        
        let (_creator, _approver, _title, _desc, votes, _target, _raised, _status, _ack) = 
            contract.get_ticket(ticket_id).unwrap();
        
        assert_eq!(votes, I256::try_from(1).unwrap());
    }

    #[test]
    fn test_close_fundraising() {
        let (_vm, mut contract) = setup();
        
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();
        
        contract.approve_ticket(ticket_id, uint!(1000_U256), U256::ZERO, uint!(9999999999_U256)).unwrap();
        
        contract.close_fundraising(ticket_id).unwrap();
        
        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.status.get().to::<u8>(), STATUS_PROJECT_PENDING);
    }

    #[test]
    fn test_mark_complete() {
        let (_vm, mut contract) = setup();
        
        let ticket_id = contract.create_ticket(mock_title(), mock_desc()).unwrap();
        
        contract.approve_ticket(ticket_id, uint!(1000_U256), U256::ZERO, uint!(9999999999_U256)).unwrap();
        contract.close_fundraising(ticket_id).unwrap();
        contract.mark_project_complete(ticket_id).unwrap();
        
        let ticket = contract.tickets.getter(ticket_id);
        assert_eq!(ticket.status.get().to::<u8>(), STATUS_COMPLETED);
    }
}