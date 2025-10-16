# Flashblocks Explorer

A real-time blockchain explorer that monitors both canonical blocks and flashblocks, featuring an interactive web dashboard.

## Running the Application

```bash
cargo run
```

The application will:
- Start the web dashboard on **http://localhost:8080**
- Connect to the blockchain WebSocket at `ws://16.163.4.133:8546`
- Connect to the flashblocks WebSocket at `ws://16.163.4.133:1111`
- Save block data to `data/{block_number}.json`
- Save flashblock data to `data/flashblock_{block_number}_{index}.json`

## API Endpoints

- `GET /` - Web dashboard (HTML)
- `GET /api/blocks` - List of latest blocks (JSON)
- `GET /api/blocks/{block_number}` - Block details with flashblocks (JSON)