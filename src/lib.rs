#![cfg_attr(not(feature = "export-abi"), no_main)]
extern crate alloc;

use stylus_sdk::{
    prelude::*,
    storage::{StorageAddress, StorageI256, StorageString, StorageU8, StorageU256},
};
use alloy_primitives::{Address, U256, I256, U8};
use alloy_sol_types::{sol, SolError}; 

// Define Solidity events and errors for the ABI
sol! {
    event TicketCreated(uint256 indexed ticket_id, address indexed creator);
    event FundraisingStarted(uint256 indexed ticket_id, uint256 target_amount, uint256 end_time);
    event FundsReceived(uint256 indexed ticket_id, address contributor, uint256 amount);
    event FundraisingClosed(uint256 indexed ticket_id, uint256 total_raised);
    event StatusChanged(uint256 indexed ticket_id, uint8 new_status);

    error NotAuthorized();
    error InvalidStatus();
    error TimeWindowInvalid();
    error InsufficientFunds();
    error TicketNotFound();
}

// Status Enums for clarity
const STATUS_PENDING: u8 = 0;
const STATUS_FUNDRAISING: u8 = 1;
const STATUS_PROJECT_PENDING: u8 = 2;
const STATUS_COMPLETED: u8 = 3;
const STATUS_ACKNOWLEDGED: u8 = 4;


#[storage]
pub struct Ticket {
    creator: StorageAddress,
    description: StorageString,
    votes: StorageI256,
    target_amount: StorageU256, 
    raised_amount: StorageU256, 
    start_time: StorageU256,    
    end_time: StorageU256,      
    status: StorageU8,
}

#[storage]
pub struct User {
    name: StorageString,
    id: StorageString,
    role: StorageU8, // 0 = student, 1 = exco
}

sol_storage! {
    #[entrypoint]
    pub struct CollegeFundraiser {
        address escrow_account;
        address owner;
        uint256 reopen_fee;
        uint256 ticket_count;

        mapping(address => bool) is_exco;
        mapping(uint256 => Ticket) tickets;
        mapping(address => User) users;
    }
}

#[public]
impl CollegeFundraiser {
    
    // --- Initialization & Admin ---

    /// Register a user as student or exco
    pub fn register_user(&mut self, name: String, id: String, role: u8) -> Result<(), Vec<u8>> {
        let sender = self.vm().msg_sender();
        let mut user = self.users.setter(sender);

        // Anyone can register as exco or student
        if role == 1 {
            self.is_exco.setter(sender).set(true);
        }

        user.name.set_str(name);
        user.id.set_str(id);
        user.role.set(U8::from(role));
        Ok(())
    }

    pub fn init(&mut self, escrow: Address, fee: U256) -> Result<(), Vec<u8>> {
        self.escrow_account.set(escrow);
        self.owner.set(self.vm().msg_sender()); 
        self.reopen_fee.set(fee);
        // The deployer is automatically an Exco
        self.is_exco.setter(self.vm().msg_sender()).set(true);
        Ok(())
    }

    pub fn add_exco(&mut self, new_exco: Address) -> Result<(), Vec<u8>> {
        self.only_owner()?;
        self.is_exco.setter(new_exco).set(true);
        Ok(())
    }

    // --- Student Features ---

    pub fn create_ticket(&mut self, description: String) -> Result<U256, Vec<u8>> {
        let ticket_id = self.ticket_count.get();
        let creator = self.vm().msg_sender();
        let mut ticket = self.tickets.setter(ticket_id);
        
        ticket.creator.set(creator);
        ticket.description.set_str(description);
        ticket.status.set(U8::from(STATUS_PENDING)); 
        ticket.votes.set(I256::ZERO);

        self.ticket_count.set(ticket_id + U256::from(1));
        
        log(self.vm(), TicketCreated { ticket_id, creator });
        Ok(ticket_id)
    }

    pub fn vote(&mut self, ticket_id: U256, upvote: bool) -> Result<(), Vec<u8>> {
        if ticket_id >= self.ticket_count.get() {
            // FIX: Use .abi_encode() instead of .encode()
            return Err(TicketNotFound{}.abi_encode()); 
        }

        let mut ticket = self.tickets.setter(ticket_id);
        
        if ticket.status.get() != U8::from(STATUS_PENDING) {
            // FIX: Use .abi_encode()
            return Err(InvalidStatus{}.abi_encode());
        }

        let current_votes = ticket.votes.get();
        let val = if upvote { I256::try_from(1).unwrap() } else { I256::try_from(-1).unwrap() };
        ticket.votes.set(current_votes + val);
        
        Ok(())
    }

    pub fn acknowledge_completion(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        let msg_sender = self.vm().msg_sender();
        let mut ticket = self.tickets.setter(ticket_id);
        
        if msg_sender != ticket.creator.get() {
            return Err(NotAuthorized{}.abi_encode());
        }
        if ticket.status.get() != U8::from(STATUS_COMPLETED) {
            return Err(InvalidStatus{}.abi_encode());
        }

        ticket.status.set(U8::from(STATUS_ACKNOWLEDGED));
        log(self.vm(), StatusChanged { ticket_id, new_status: STATUS_ACKNOWLEDGED });
        Ok(())
    }

    // --- Exco Features ---

    pub fn approve_ticket(&mut self, ticket_id: U256, target_amount: U256, start_time: U256, end_time: U256) -> Result<(), Vec<u8>> {
        self.only_exco()?;
        let mut ticket = self.tickets.setter(ticket_id);

        if ticket.status.get() != U8::from(STATUS_PENDING) {
             return Err(InvalidStatus{}.abi_encode());
        }

        ticket.target_amount.set(target_amount);
        ticket.start_time.set(start_time);
        ticket.end_time.set(end_time);
        ticket.status.set(U8::from(STATUS_FUNDRAISING));

        log(self.vm(), FundraisingStarted { ticket_id, target_amount, end_time });
        Ok(())
    }

    pub fn close_fundraising(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        self.only_exco()?;
        let escrow = self.escrow_account.get();
        let amount_raised = {
            let ticket = self.tickets.getter(ticket_id);
            if ticket.status.get() != U8::from(STATUS_FUNDRAISING) {
                return Err(InvalidStatus{}.abi_encode());
            }
            ticket.raised_amount.get()
        };

        if amount_raised > U256::ZERO {
            let _ = self.vm().transfer_eth(escrow, amount_raised); 
        }

        let mut ticket = self.tickets.setter(ticket_id);
        ticket.status.set(U8::from(STATUS_PROJECT_PENDING)); 
        log(self.vm(), FundraisingClosed { ticket_id, total_raised: amount_raised });
        Ok(())
    }

    #[payable]
    pub fn reopen_fundraising(&mut self, ticket_id: U256, new_target: U256, new_end_date: U256) -> Result<(), Vec<u8>> {
        self.only_exco()?;
        
        let msg_value = self.vm().msg_value();
        let escrow = self.escrow_account.get();
        let reopen_fee = self.reopen_fee.get();
        
        if msg_value < reopen_fee {
             return Err(InsufficientFunds{}.abi_encode());
        }

        let _ = self.vm().transfer_eth(escrow, msg_value);

        let mut ticket = self.tickets.setter(ticket_id);
        
        if ticket.status.get() != U8::from(STATUS_PROJECT_PENDING) {
            return Err(InvalidStatus{}.abi_encode());
        }

        ticket.target_amount.set(new_target);
        ticket.end_time.set(new_end_date);
        ticket.status.set(U8::from(STATUS_FUNDRAISING)); 

        log(self.vm(), StatusChanged { ticket_id, new_status: STATUS_FUNDRAISING });
        Ok(())
    }

    pub fn mark_project_complete(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        self.only_exco()?;
        let mut ticket = self.tickets.setter(ticket_id);

        if ticket.status.get() != U8::from(STATUS_PROJECT_PENDING) {
            return Err(InvalidStatus{}.abi_encode());
        }

        ticket.status.set(U8::from(STATUS_COMPLETED));
        log(self.vm(), StatusChanged { ticket_id, new_status: STATUS_COMPLETED });
        Ok(())
    }

    // --- Public Features ---

    #[payable]
    pub fn fund_ticket(&mut self, ticket_id: U256) -> Result<(), Vec<u8>> {
        let msg_sender = self.vm().msg_sender();
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

        log(self.vm(), FundsReceived { ticket_id, contributor: msg_sender, amount: msg_value });
        Ok(())
    }

    // --- View Functions (Getters) ---
    
    pub fn get_ticket(&self, ticket_id: U256) -> Result<(Address, String, I256, U256, U256, u8), Vec<u8>> {
        let ticket = self.tickets.getter(ticket_id);
        Ok((
            ticket.creator.get(),
            ticket.description.get_string(),
            ticket.votes.get(),
            ticket.target_amount.get(),
            ticket.raised_amount.get(),
            ticket.status.get().to(), 
        ))
    }

    pub fn get_user(&self, user_address: Address) -> Result<(String, String, u8), Vec<u8>> {
        let user = self.users.getter(user_address);
        Ok((
            user.name.get_string(),
            user.id.get_string(),
            user.role.get().to(),
        ))
    }

    pub fn get_ticket_count(&self) -> U256 {
        self.ticket_count.get()
    }

    pub fn get_escrow_account(&self) -> Address {
        self.escrow_account.get()
    }

    pub fn get_reopen_fee(&self) -> U256 {
        self.reopen_fee.get()
    }

    pub fn is_user_exco(&self, user_address: Address) -> bool {
        self.is_exco.get(user_address)
    }

    pub fn get_owner(&self) -> Address {
        self.owner.get()
    }
}

// Internal Helpers
impl CollegeFundraiser {
    fn only_owner(&mut self) -> Result<(), Vec<u8>> {
        if self.vm().msg_sender() != self.owner.get() {
            return Err(NotAuthorized{}.abi_encode());
        }
        Ok(())
    }

    fn only_exco(&mut self) -> Result<(), Vec<u8>> {
        if !self.is_exco.get(self.vm().msg_sender()) {
            return Err(NotAuthorized{}.abi_encode());
        }
        Ok(())
    }
}