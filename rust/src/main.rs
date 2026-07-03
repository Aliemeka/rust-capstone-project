#![allow(unused)]
use bitcoin::hex::DisplayHex;
use bitcoincore_rpc::bitcoin::{Amount, Network};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use serde::Deserialize;
use serde_json::json;
use std::fs::File;
use std::io::Write;

// Node access params
const RPC_URL: &str = "http://127.0.0.1:18443"; // Default regtest RPC port
const RPC_USER: &str = "alice";
const RPC_PASS: &str = "password";

// You can use calls not provided in RPC lib API using the generic `call` function.
// An example of using the `send` RPC call, which doesn't have exposed API.
// You can also use serde_json `Deserialize` derivation to capture the returned json result.
fn send(rpc: &Client, addr: &str) -> bitcoincore_rpc::Result<String> {
    let args = [
        json!([{addr : 100 }]), // recipient address
        json!(null),            // conf target
        json!(null),            // estimate mode
        json!(null),            // fee rate in sats/vb
        json!(null),            // Empty option object
    ];

    #[derive(Deserialize)]
    struct SendResult {
        complete: bool,
        txid: String,
    }
    let send_result = rpc.call::<SendResult>("send", &args)?;
    assert!(send_result.complete);
    Ok(send_result.txid)
}

// Build an RPC client scoped to a specific wallet (routes wallet RPCs to it).
fn wallet_client(wallet: &str) -> bitcoincore_rpc::Result<Client> {
    Client::new(
        &format!("{RPC_URL}/wallet/{wallet}"),
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )
}

// Create the wallet, or load it if it already exists on disk (idempotent).
fn create_or_load_wallet(rpc: &Client, name: &str) -> bitcoincore_rpc::Result<()> {
    if rpc.list_wallets()?.iter().any(|w| w == name) {
        return Ok(()); // already loaded
    }
    if rpc.load_wallet(name).is_err() {
        rpc.create_wallet(name, None, None, None, None)?;
    }
    Ok(())
}

fn main() -> bitcoincore_rpc::Result<()> {
    // Connect to Bitcoin Core RPC
    let rpc = Client::new(
        RPC_URL,
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;

    // Get blockchain info
    let blockchain_info = rpc.get_blockchain_info()?;
    println!("Blockchain Info: {:?}", blockchain_info);

    // Create/Load the wallets, named 'Miner' and 'Trader'. Have logic to optionally create/load them if they do not exist or not loaded already.
    create_or_load_wallet(&rpc, "Miner")?;
    create_or_load_wallet(&rpc, "Trader")?;
    let miner = wallet_client("Miner")?;
    let trader = wallet_client("Trader")?;

    // Generate spendable balances in the Miner wallet. How many blocks needs to be mined?
    // Miner needs an address to receive block rewards, labeled "Mining Reward".
    // get_new_address returns a network-unchecked address; assert it's regtest.
    let mining_address = miner
        .get_new_address(Some("Mining Reward"), None)?
        .require_network(Network::Regtest)
        .unwrap();

    miner.generate_to_address(101, &mining_address)?;

    let balance = miner.get_balance(None, None)?;
    println!("Miner balance: {balance}");

    // Load Trader wallet and generate a new address
    let trader_address = trader
        .get_new_address(Some("Received"), None)?
        .require_network(Network::Regtest)
        .unwrap();

    // Send 20 BTC from Miner to Trader
    // Pay 20 BTC to the Trader. The one 50 BTC UTXO funds it => 1 input,
    // 2 outputs (20 to Trader + change back to Miner).
    let txid = miner.send_to_address(
        &trader_address,
        Amount::from_btc(20.0)?,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    println!("Sent 20 BTC, txid: {txid}");

    // Check transaction in mempool
    let mempool_entry: serde_json::Value =
        miner.call("getmempoolentry", &[json!(txid.to_string())])?;
    println!("Mempool entry: {mempool_entry:#}");

    // Mine 1 block to confirm the transaction
    miner.generate_to_address(1, &mining_address)?;

    // Extract all required transaction details
    // Wallet-level info: fee (negative), confirming block hash + height.
    let gt = miner.get_transaction(&txid, None)?;
    let fee = gt.fee.map(|f| f.to_btc()).unwrap_or(0.0);
    let block_hash = gt.info.blockhash.expect("tx should be confirmed");
    let block_height = gt.info.blockheight.expect("tx should be confirmed");

    // Verbose gettransaction => the decoded tx, so we can inspect vin/vout.
    let tx_json: serde_json::Value = miner.call(
        "gettransaction",
        &[json!(txid.to_string()), json!(null), json!(true)],
    )?;
    let vout = tx_json["decoded"]["vout"].as_array().unwrap();
    let vin = tx_json["decoded"]["vin"].as_array().unwrap();

    // Trader output = the vout paying the trader address; the other is change.
    let trader_str = trader_address.to_string();
    let trader_out = vout
        .iter()
        .find(|o| o["scriptPubKey"]["address"] == json!(trader_str))
        .expect("trader output");
    let change_out = vout
        .iter()
        .find(|o| o["scriptPubKey"]["address"] != json!(trader_str))
        .expect("change output");

    let trader_out_addr = trader_out["scriptPubKey"]["address"].as_str().unwrap();
    let trader_out_amount = trader_out["value"].as_f64().unwrap();
    let change_addr = change_out["scriptPubKey"]["address"].as_str().unwrap();
    let change_amount = change_out["value"].as_f64().unwrap();

    // Miner input = the previous output (coinbase) this tx spends. Look it up.
    let prev_txid = vin[0]["txid"].as_str().unwrap();
    let prev_vout = vin[0]["vout"].as_u64().unwrap() as usize;
    let prev: serde_json::Value =
        miner.call("getrawtransaction", &[json!(prev_txid), json!(true)])?;
    let prev_out = &prev["vout"][prev_vout];
    let miner_in_addr = prev_out["scriptPubKey"]["address"].as_str().unwrap();
    let miner_in_amount = prev_out["value"].as_f64().unwrap();

    // Write the data to ../out.txt in the specified format given in readme.md
    let mut file = File::create("../out.txt")?;
    writeln!(file, "{txid}")?;
    writeln!(file, "{miner_in_addr}")?;
    writeln!(file, "{miner_in_amount}")?;
    writeln!(file, "{trader_out_addr}")?;
    writeln!(file, "{trader_out_amount}")?;
    writeln!(file, "{change_addr}")?;
    writeln!(file, "{change_amount}")?;
    writeln!(file, "{fee}")?;
    writeln!(file, "{block_height}")?;
    writeln!(file, "{block_hash}")?;

    Ok(())
}
