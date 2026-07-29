// Prototype RadioDoge firmware for the Heltec WiFi LoRa 32 v2 & v3 modules
// v0.3.8 — Bug fixes + features: fake-neighbor fix, OOB write fix, KNOWN_CMDS 0x24/0x25,
//           battery voltage (0x26), board MAC address (0x27), accumulator drain fix.
#include <Wire.h>
#include "LoRaWan_APP.h"
#include "Arduino.h"
#include <Adafruit_GFX.h>
#include <Adafruit_SSD1306.h>
#include <WiFi.h>
#include <WebServer.h>
#include <DNSServer.h>
#include <WiFiUdp.h>
#include "radioDogeTypes.h"
#include "Images/logoImage.h"
#include "Images/coin.h"
#include "Images/doge.h"
#include "Images/sendingDogeCoin.h"
#include "Images/receivingDogeCoin.h"
#include "esp_mac.h"

// Forward-declare the local node address global so setupBLE() (defined inside the
// #if ENABLE_BLE block near the top) can access its members before the variable's
// full definition later in the file.  radioDogeTypes.h (included above) provides
// the complete nodeAddress struct, so the compiler can resolve member accesses.
extern nodeAddress local;

// v0.3.6 — BLE Nordic UART Service (requires ESP32 BLE Arduino library)
// v0.3.13 — UUIDs confirmed aligned with RadioDoge GUI connection-bridge.ts.
//   NUS RX (host → board, WRITE)  : 6E400002-B5A3-F393-E0A9-E50E24DCCA9E
//   NUS TX (board → host, NOTIFY) : 6E400003-B5A3-F393-E0A9-E50E24DCCA9E
// To disable BLE, set ENABLE_BLE to false below.
#define ENABLE_BLE true
#if ENABLE_BLE
  #include <BLEDevice.h>
  #include <BLEServer.h>
  #include <BLEUtils.h>
  #include <BLE2902.h>
  #define BLE_DEVICE_NAME_PREFIX "RadioDoge"
  // Nordic UART Service (NUS) UUIDs — aligned with GUI v0.3.13 connection-bridge.ts
  #define NORDIC_UART_SERVICE_UUID    "6E400001-B5A3-F393-E0A9-E50E24DCCA9E"
  #define NORDIC_UART_CHAR_RX_UUID    "6E400002-B5A3-F393-E0A9-E50E24DCCA9E"
  #define NORDIC_UART_CHAR_TX_UUID    "6E400003-B5A3-F393-E0A9-E50E24DCCA9E"
  // BLE state globals — must be declared before the callback classes that reference them
  bool bleDeviceConnected = false;
  uint8_t bleRxBuffer[256];
  int bleRxLen = 0;
  bool blePendingData = false;
  // v0.4.1 — millis() of the most recent BLE write. A large packet can be split
  // across several GATT writes, and the desktop protocol has no in-band length
  // for CMD_DOGE_TX / CMD_REQUEST_BALANCE, so the framer waits for a short quiet
  // gap before treating a variable-length packet as complete.
  unsigned long lastBleRxMillis = 0;
  BLEServer *pBleServer = NULL;
  BLECharacteristic *pBleTxChar = NULL;
  class BleServerCallbacks : public BLEServerCallbacks {
    void onConnect(BLEServer* s)    { bleDeviceConnected = true;  }
    void onDisconnect(BLEServer* s) { bleDeviceConnected = false; BLEDevice::startAdvertising(); }
  };
  class BleRxCallbacks : public BLECharacteristicCallbacks {
    void onWrite(BLECharacteristic *pChar) {
      String v = pChar->getValue();
      for (size_t i = 0; i < v.length() && bleRxLen < 255; i++)
        bleRxBuffer[bleRxLen++] = (uint8_t)v[i];
      blePendingData = true;
      lastBleRxMillis = millis();
    }
  };
  // Send a response packet over BLE TX characteristic (notify)
  void bleSend(uint8_t* data, size_t len) {
    if (bleDeviceConnected && pBleTxChar) {
      pBleTxChar->setValue(data, len);
      pBleTxChar->notify();
    }
  }
  void setupBLE() {
    // v0.3.7 — Build device name: "RadioDoge-12.3.4" using node address for clarity
    // (avoids "Ra" truncation; full name fits BLE 29-byte limit; matches node ID in GUI)
    char bleName[32];
    snprintf(bleName, sizeof(bleName), "%s-%d.%d.%d",
             BLE_DEVICE_NAME_PREFIX, local.region, local.community, local.node);
    BLEDevice::init(bleName);
    pBleServer = BLEDevice::createServer();
    pBleServer->setCallbacks(new BleServerCallbacks());
    BLEService *pSvc = pBleServer->createService(NORDIC_UART_SERVICE_UUID);
    pBleTxChar = pSvc->createCharacteristic(NORDIC_UART_CHAR_TX_UUID, BLECharacteristic::PROPERTY_NOTIFY);
    pBleTxChar->addDescriptor(new BLE2902());
    BLECharacteristic *pRxChar = pSvc->createCharacteristic(NORDIC_UART_CHAR_RX_UUID, BLECharacteristic::PROPERTY_WRITE);
    pRxChar->setCallbacks(new BleRxCallbacks());
    pSvc->start();
    BLEAdvertising *pAdv = BLEDevice::getAdvertising();
    pAdv->addServiceUUID(NORDIC_UART_SERVICE_UUID);
    pAdv->setScanResponse(false);
    pAdv->setMinPreferred(0x06);
    BLEDevice::startAdvertising();
    Serial.println("BLE started: " + String(bleName));
  }
  // v0.4.0 — Enable/disable BLE advertising at runtime (CMD_BLE_TOGGLE 0x28).
  // A connected central stays connected when advertising stops; new centrals
  // simply can no longer discover the board.
  void bleSetAdvertising(bool enable) {
    if (pBleServer == NULL) {
      if (enable) setupBLE();
      return;
    }
    if (enable) {
      BLEDevice::startAdvertising();
    } else {
      BLEDevice::stopAdvertising();
    }
  }
#else
  void bleSend(uint8_t*, size_t) {}
  void setupBLE() {}
  void bleSetAdvertising(bool) {}
#endif

// V3 Display Configuration
#define SCREEN_WIDTH 128
#define SCREEN_HEIGHT 64
#define OLED_RESET 21
#define SCREEN_ADDRESS 0x3C
#define SDA_PIN 17
#define SCL_PIN 18
#define VEXT_PIN 36  // Controls display power on V3
#define TEST_COIN_AMOUNT 1234.56

// WiFi Configuration
const char* ap_ssid = "RadioDoge";  // Access Point name
String ap_password = "radiodoge";  // AP password (now configurable)
WebServer server(80);  // Web server on port 80

// Internet WiFi Configuration (configurable via web interface)
String internet_ssid = "";
String internet_password = "";
bool internet_connected = false;
bool dual_wifi_mode = false;

// Internet Bridging Configuration
bool internet_bridge_enabled = false;
DNSServer dnsServer;
WiFiUDP udp;
IPAddress ap_gateway(192, 168, 4, 1);
IPAddress ap_subnet(255, 255, 255, 0);
IPAddress ap_dns(192, 168, 4, 1);
IPAddress ap_ip_start(192, 168, 4, 2);
IPAddress ap_ip_end(192, 168, 4, 10);

// NAT Router Configuration
struct ClientInfo {
  IPAddress ip;
  IPAddress gateway;
  unsigned long lastSeen;
  bool active;
};

ClientInfo connectedClients[8];
int clientCount = 0;
unsigned long lastClientCheck = 0;

// Gateway Configuration (persistent storage for all types)
String gateway_type = "none";
String gateway_ip = "";
String gateway_port = "";
String gateway_endpoint = "";
String gateway_username = "";
String gateway_password = "";

// v0.3.6 — Board-is-source-of-truth gateway mode flag (persisted to NVS)
bool gateway_mode = false;
// v0.3.7 — WiFi on/off toggle (persisted to NVS, default on)
bool wifi_enabled = true;

// v0.4.0 — Board-is-source-of-truth BLE advertising flag (persisted to NVS)
bool ble_enabled = true;
// v0.3.7 — Duplicate node address detection
bool addrConflict = false;
unsigned long addrConflictNotifiedAt = 0;

// Password requirements
const int MIN_PASSWORD_LENGTH = 8;
const int MAX_PASSWORD_LENGTH = 32;

// Real-time logging system
const int MAX_LOG_ENTRIES = 100;
const int MAX_LOG_LENGTH = 200;
String logBuffer[MAX_LOG_ENTRIES];
int logIndex = 0;
int logCount = 0;

// HTTP Client for internet requests
#include <HTTPClient.h>
#include <WiFiClient.h>
#include <nvs.h>
#include <nvs_flash.h>


// Global display object
Adafruit_SSD1306 radioDogeDisplay(SCREEN_WIDTH, SCREEN_HEIGHT, &Wire, OLED_RESET);

#define RF_FREQUENCY 915000000   // Hz
#define TX_OUTPUT_POWER 5        // dBm
#define LORA_BANDWIDTH 0         // [0: 125 kHz, \
                                 //  1: 250 kHz, \
                                 //  2: 500 kHz, \
                                 //  3: Reserved]
#define LORA_SPREADING_FACTOR 7  // [SF7..SF12]
#define LORA_CODINGRATE 1        // [1: 4/5, \
                                 //  2: 4/6, \
                                 //  3: 4/7, \
                                 //  4: 4/8]
// v0.4.1 — Runtime-adjustable radio parameters (CMD_SET_LORA_PARAMS 0x21).
// The #defines above remain the power-on defaults; these mirror them and are
// what SetTxConfig/SetRxConfig actually read, so the host can retune the radio
// without a reflash. Persisted to NVS, so a retuned board comes back retuned.
uint32_t lora_freq_hz  = RF_FREQUENCY;
int8_t   lora_tx_power = TX_OUTPUT_POWER;
uint8_t  lora_bandwidth        = LORA_BANDWIDTH;
uint8_t  lora_spreading_factor = LORA_SPREADING_FACTOR;
uint8_t  lora_coding_rate      = LORA_CODINGRATE;

#define LORA_PREAMBLE_LENGTH 8   // Same for Tx and Rx
#define LORA_SYMBOL_TIMEOUT 0    // Symbols
#define LORA_FIX_LENGTH_PAYLOAD_ON false
#define LORA_IQ_INVERSION_ON false
#define RX_TIMEOUT_VALUE 1000
#define SERIAL_HEADER_SIZE 2
// v0.4.x — over-the-air single-packet header: [type, flags, src(3), dst(3)] = 8 bytes.
#define SINGLE_PACKET_HEADER_SIZE 8
#define BUFFER_SIZE 256  // Define the payload size here
#define CONTROL_SIZE 8
#define SERIAL_TERMINATOR 255
#define SENDER_ADDRESS_OFFSET 2
#define COMMAND_ACK_CODE 6 // 0x06 = 'ACK'
#define COMMAND_NACK_CODE 21 // 0x15 = 'NAK'

// Multipart packet constants
#define MAX_MULTIPART_PARTS 20
// How many senders may have a sequence in flight at once. Distinct from
// MAX_MULTIPART_PARTS, which is how many parts one sequence has; they happen to
// share a value. Each session reserves its own 4000-byte reassembly buffer.
#define MAX_MULTIPART_SESSIONS 20
#define MULTIPART_CHUNK_SIZE 200  // Leave room for headers
#define MULTIPART_TIMEOUT_MS 30000  // 30 seconds timeout for reassembly
#define MULTIPART_HEADER_SIZE 12  // Packet type + dest + src + part info

// ─── v0.4.2 — Desktop-protocol multipart (host <-> board, and on the air) ─────
//
// Distinct from the firmware's own MULTIPART_PACKET ('m') format above. A
// desktop multipart frame is an ordinary 8-byte desktop header whose flags byte
// has 0x1 in its low nibble, followed by five bytes the board never interprets
// and forwards verbatim:
//
//   [0..8]  cmd, flags, src(3), dst(3)
//   [8]     total parts
//   [9]     part index (0-based)
//   [10-11] session id (big-endian u16, same for every part)
//   [12]    chunk length — how many payload bytes follow
//   [13..]  chunk
//
// The chunk-length byte is what makes the frame self-delimiting on a byte
// stream. Serial has no packet boundaries, so without it neither this firmware
// nor the receiving host could tell where a short final part ended, and the
// frame carrying a signed transaction was framed by guesswork.
#define DESKTOP_FLAG_MULTIPART 0x01
#define DESKTOP_MULTIPART_EXTRA 5    // [total, index, sess_hi, sess_lo, chunk_len]
#define DESKTOP_MULTIPART_CHUNK_MAX 187  // 192 - DESKTOP_MULTIPART_EXTRA

// How long to wait for the remainder of a host frame whose length is known.
#define HOST_FRAME_TIMEOUT_MS 1000
// A variable-length host frame carries no length, so a gap in the byte stream
// marks its end. At 115200 baud consecutive bytes are ~0.09 ms apart, so 30 ms
// of silence is several hundred byte-times of margin.
#define HOST_FRAME_QUIET_MS 30
// How long to wait for a variable-length payload to start arriving at all.
#define HOST_PAYLOAD_START_TIMEOUT_MS 250
// Upper bound on how long a single LoRa transmission may take before the board
// gives up waiting for it. Sized for the slowest configuration the radio can be
// retuned to (SF12, 125 kHz, a full 200-byte packet is ~6 s of airtime).
#define LORA_TX_TIMEOUT_MS 10000

// Mesh networking configuration
#define ENABLE_MESH_REBROADCAST true
// v0.4.0 — Maximum number of mesh hops a multipart broadcast may traverse before
// a relay drops it instead of rebroadcasting. The hop count travels in the
// multipart header's previously-unused `reserved` byte (offset 10) and is
// incremented on each rebroadcast. Bounds mesh storms independently of the
// dedup table (which only suppresses exact duplicates).
#define MAX_REBROADCAST_HOPS 3
#define HOST_ACK_NACK_SIZE 3

// v0.4.2 — Echo the log ring buffer to the serial port as well as the web UI.
//
// Off by default: the host speaks a binary protocol on that wire, and text
// interleaved with packets can be mistaken for a packet header, costing the real
// packet behind it. Set to 1 when debugging with a serial monitor and no host
// software attached — never in a build that will talk to the app or the daemon.
#define HOST_SERIAL_DEBUG 0

// Request queuing and confirmation system
#define MAX_PENDING_REQUESTS 10
#define CONFIRMATION_TIMEOUT_MS 15000  // 15 seconds timeout for confirmations
// v0.4.2 — Raised from 30 s. This is measured from the moment a request is
// *queued*, not from when it starts, and a request ahead of it in the queue can
// hold the line for a full CONFIRMATION_TIMEOUT_MS. At 30 s only the first two
// of the ten queue slots could ever be reached: everything behind them expired
// untried, and the API had already reported them accepted. The bound must exceed
// MAX_PENDING_REQUESTS x CONFIRMATION_TIMEOUT_MS (10 x 15 s) for a full queue to
// drain, with margin for the airtime each request spends transmitting.
#define REQUEST_TIMEOUT_MS 210000      // 3.5 minutes: a full queue can drain before anything expires

// Request types
enum RequestType {
  REQUEST_BROADCAST,
  REQUEST_TRANSACTION,
  REQUEST_MESSAGE,
  REQUEST_PING
};

// Request states
enum RequestState {
  REQUEST_IDLE,
  REQUEST_WAITING_FOR_CONFIRMATION,
  REQUEST_PROCESSING_QUEUE,
  REQUEST_TIMEOUT
};

// Pending request structure
struct PendingRequest {
  RequestType type;
  String message;
  String typeStr;        // For broadcasts: "transaction", "announcement", etc.
  String priority;       // For broadcasts: "normal", "high", etc.
  nodeAddress destination; // For direct messages/transactions
  unsigned long timestamp;
  bool isMultipart;
  bool requiresConfirmation;
  String requestId;      // Unique identifier for tracking
};
// Bumped whenever the host↔board contract changes. The host reads this back
// from GET_FIRMWARE_VERSION and uses it to decide what it may send:
// radio::MIN_MULTIPART_FIRMWARE (11) is the build that frames a host→board
// multipart sequence correctly, so only from 11 onward will a host split a
// transaction that does not fit in one packet.
#define FIRMWARE_VERSION 11  // v0.4.2

#ifdef WIFI_LoRa_32_V2
#define HELTEC_BOARD_VERSION 2
#else
#define HELTEC_BOARD_VERSION 3
#endif

char txPacket[BUFFER_SIZE];
char rxPacket[BUFFER_SIZE];
uint8_t controlPacket[CONTROL_SIZE];
uint8_t serialBuf[BUFFER_SIZE];
uint8_t hostReplyBuf[BUFFER_SIZE];

// Multipart packet structures
struct MultipartPacket {
  uint8_t packetType;
  uint8_t destRegion;
  uint8_t destCommunity;
  uint8_t destNode;
  uint8_t srcRegion;
  uint8_t srcCommunity;
  uint8_t srcNode;
  uint8_t partNumber;
  uint8_t totalParts;
  uint8_t dataType;  // 0=transaction, 1=message, 2=broadcast
  uint8_t reserved;
  char data[MULTIPART_CHUNK_SIZE];
};

struct MultipartReassembly {
  uint8_t srcRegion;
  uint8_t srcCommunity;
  uint8_t srcNode;
  uint8_t totalParts;
  uint8_t receivedParts;
  uint8_t dataType;
  uint8_t hops;          // v0.4.0 — mesh hop count from the multipart `reserved` byte
  unsigned long startTime;
  char assembledData[MAX_MULTIPART_PARTS * MULTIPART_CHUNK_SIZE];
  bool partsReceived[MAX_MULTIPART_PARTS];
  uint16_t partSizes[MAX_MULTIPART_PARTS]; // Track actual size of each part
};

// Global multipart reassembly buffer
MultipartReassembly multipartBuffer[MAX_MULTIPART_SESSIONS];
int activeMultipartSessions = 0;
uint8_t serialHeader[SERIAL_HEADER_SIZE];
uint8_t hostCommandReply[BUFFER_SIZE];
uint8_t hostACK[3] = {RESULT_CODE, 1, COMMAND_ACK_CODE};
uint8_t hostNACK[3] = {RESULT_CODE, 1, COMMAND_NACK_CODE};

// Request queuing and confirmation system globals
RequestState currentRequestState = REQUEST_IDLE;
PendingRequest pendingRequests[MAX_PENDING_REQUESTS];
int pendingRequestCount = 0;
unsigned long confirmationStartTime = 0;
String currentRequestId = "";
int requestIdCounter = 0;

static RadioEvents_t RadioEvents;
void OnTxDone(void);
void OnTxTimeout(void);
void OnRxDone(uint8_t *payload, uint16_t messageSize, int16_t rssiMeasured, int8_t snr);

int16_t rssi;
int16_t rxSize;
int8_t lastSnr = 0;      // v0.3.6 — for OLED status cycle
bool isLoRaIdle = true;
bool needToSendACK = false;

// v0.4.2 — True from the moment Radio.Send() is called until TxDone or
// TxTimeout fires.
//
// `isLoRaIdle` cannot serve this purpose: it also means "nothing is happening,
// put the radio back into receive", and the main loop acts on it. Any path that
// set it true straight after Radio.Send() therefore had the next loop iteration
// call Radio.Rx(0) a couple of milliseconds into a transmission that needs
// hundreds — which aborts the transmission on the SX1262. Every desktop
// command, including the one that carries a signed transaction, did exactly
// that. Transmissions now go through SendLoRaAndWait(), which watches this flag.
volatile bool loRaTxPending = false;

// v0.3.6 — OLED status cycle state
unsigned long lastStatusCycle = 0;
int statusPage = 0;
bool showTxOk = false;
unsigned long txOkTimestamp = 0;
uint32_t pktRxCount = 0;
uint32_t pktTxCount = 0;

// Broadcast deduplication — suppress mesh storm re-delivery
#define MAX_SEEN_BROADCASTS 20
#define SEEN_BROADCAST_TTL_MS 120000UL  // 2 minutes

struct SeenBroadcast {
  uint8_t srcRegion;
  uint8_t srcCommunity;
  uint8_t srcNode;
  uint32_t dataHash;
  unsigned long seenAt;
};

SeenBroadcast seenBroadcasts[MAX_SEEN_BROADCASTS];
int seenBroadcastCount = 0;

nodeAddress local;
nodeAddress dest;
nodeAddress senderAddress;

void setup() {
  // v0.4.2 — Enlarge the UART receive FIFO before opening the port.
  //
  // A multipart transaction is a burst of ~200-byte frames, and the board does
  // not read serial while the radio is transmitting. The default 256-byte FIFO
  // is barely one frame, so anything queued behind the one being sent was
  // dropped with no error on either side. 1 KiB holds several frames, which
  // turns a host that sends slightly too fast into a delay rather than a
  // corrupted transaction. Must precede Serial.begin() to take effect.
  Serial.setRxBufferSize(1024);
  Serial.begin(115200);

  // CRITICAL: Set Vext pin LOW FIRST for V3 display power
  pinMode(VEXT_PIN, OUTPUT);
  digitalWrite(VEXT_PIN, LOW);
  delay(100); // Give display time to power up
  
  // Initialize I2C with correct V3 pins
  Wire.begin(SDA_PIN, SCL_PIN);
  
  // Initialize display
  if(!radioDogeDisplay.begin(SSD1306_SWITCHCAPVCC, SCREEN_ADDRESS)) {
    Serial.println(F("SSD1306 allocation failed"));
    for(;;);
  }
  Serial.println("Display initialized successfully!");
  
  Mcu.begin(HELTEC_BOARD_VERSION, 0);
  rssi = 0;

  RadioEvents.TxDone = OnTxDone;
  RadioEvents.TxTimeout = OnTxTimeout;
  RadioEvents.RxDone = OnRxDone;
  RadioEvents.RxTimeout = OnRxTimeout;

  Radio.Init(&RadioEvents);
  applyLoRaRadioConfig();

  delay(100);

  // Initialize NVS and load stored configurations
  initNVS();
  // v0.4.1 — Restore any host-tuned radio parameters and re-apply them, so a
  // retuned board comes back on the same channel it was retuned to.
  if (loadLoRaRadioParams()) {
    applyLoRaRadioConfig();
    addLog("[LoRa] Restored tuned radio params from NVS");
  }
  
  // Load stored AP password
  if (loadAPPassword()) {
    Serial.println("AP password restored from storage");
  } else {
    Serial.println("Using default AP password");
  }
  
  // Load stored LoRa configuration
  if (loadLoRaConfiguration()) {
    Serial.println("LoRa configuration restored from storage");
  } else {
    Serial.println("Using default LoRa configuration");
  }
  
  // Load stored gateway credentials
  if (loadGatewayCredentials()) {
    Serial.println("Gateway credentials restored from storage");
  } else {
    Serial.println("No gateway credentials found");
  }

  // v0.3.6 — Load gateway_mode from NVS (board is source of truth)
  if (loadGatewayMode()) {
    Serial.println("Gateway mode restored: " + String(gateway_mode ? "ON" : "OFF"));
  }
  // v0.3.7 — Load wifi_enabled from NVS
  loadWifiEnabled();
  if (!wifi_enabled) {
    WiFi.disconnect(true);
    WiFi.mode(WIFI_OFF);
    Serial.println("WiFi disabled (stored in NVS)");
  }

  // Initialize control messages with loaded/default address
  InitControlMessages();

  // v0.4.0 — Load ble_enabled from NVS; only start BLE if enabled
  loadBleEnabled();
  if (ble_enabled) {
    // v0.3.6 — Start BLE Nordic UART service
    setupBLE();
  } else {
    Serial.println("BLE disabled (stored in NVS)");
  }
  
  // Setup WiFi in dual mode (AP + Station)
  setupDualWiFi();
  
  
  // Setup web server routes
  setupWebServer();
  
  // v0.3.7 — Boot splash: show "RadioDoge v0.3.7" for 3.5 s then hand off to status cycle
  DrawBootSplash();
  delay(3500);

  Serial.printf("RadioDoge NV%dFW%02d initialized!\n", HELTEC_BOARD_VERSION, FIRMWARE_VERSION);
  Serial.println("Connect to WiFi: RadioDoge");
  // The AP password is deliberately not printed — it is configurable, and the
  // line was stale anyway once a user changed it.
  Serial.println("Open browser: http://192.168.4.1");
  Serial.println("Use WiFi web interface for mobile access");
}

void loop() {
  // Handle web server requests
  server.handleClient();

  // Handle internet bridge (DNS requests)
  handleInternetBridge();

  // Cleanup expired multipart sessions
  CleanupExpiredMultipartSessions();

  // Check request timeouts
  CheckRequestTimeouts();

  // v0.3.6 — Cycle OLED status pages every 3 seconds
  updateStatusDisplay();

  // v0.4.1 — Execute any command written over BLE. Previously these bytes were
  // buffered and never read, so BLE was receive-only.
  ProcessBleCommands();

  CommandAndControlLoop();
}

// V3 Display Functions
void DisplayStartupSequence() {
  // Show some animations
  CoinAnimation();
  DogeAnimation();
  // Show the send and receive images
  DrawSendingCoinsImage(TEST_COIN_AMOUNT);
  delay(2500);
  DrawReceivingCoinsImage(TEST_COIN_AMOUNT);
  delay(2500);
  // Display RadioDoge Image on startup
  DrawRadioDogeLogo();
}

// Simple display functions for V3
void DisplayTXMessage(String message, nodeAddress dest) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setCursor(0, 0);
  radioDogeDisplay.println("TX: " + message);
  radioDogeDisplay.setCursor(0, 20);
  radioDogeDisplay.println("To: " + String(dest.region) + "." + String(dest.community) + "." + String(dest.node));
  radioDogeDisplay.display();
  addDisplayLog("TX: " + message + " to " + String(dest.region) + "." + String(dest.community) + "." + String(dest.node));
}

void DisplayRXMessage(String message, nodeAddress sender) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setCursor(0, 0);
  radioDogeDisplay.println("RX: " + message);
  radioDogeDisplay.setCursor(0, 20);
  radioDogeDisplay.println("From: " + String(sender.region) + "." + String(sender.community) + "." + String(sender.node));
  radioDogeDisplay.display();
  addDisplayLog("RX: " + message + " from " + String(sender.region) + "." + String(sender.community) + "." + String(sender.node));
}

void DisplayBroadcastMessage(String message, nodeAddress sender) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setCursor(0, 0);
  radioDogeDisplay.println("BROADCAST: " + message);
  radioDogeDisplay.setCursor(0, 20);
  radioDogeDisplay.println("From: " + String(sender.region) + "." + String(sender.community) + "." + String(sender.node));
  radioDogeDisplay.display();
  addDisplayLog("BROADCAST: " + message + " from " + String(sender.region) + "." + String(sender.community) + "." + String(sender.node));
}

// v0.3.7 — Boot splash: shown for 3.5 s before handing off to status cycle
void DrawBootSplash() {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  // Doge logo at top
  radioDogeDisplay.drawXBitmap(48, 0, doge_bits, doge_width, doge_height, SSD1306_WHITE);
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setCursor(18, 38);
  radioDogeDisplay.println("RadioDoge v0.3.7");
  radioDogeDisplay.setCursor(10, 52);
  radioDogeDisplay.println("Much mesh. Very wow.");
  radioDogeDisplay.display();
}

// v0.3.7 — drawStatusScreen(): unified status display with icon row, no raw debug text.
// Page 0: Node address + icon row (GW/WiFi/BLE) + "ADDR CONFLICT!" banner
// Page 1: RSSI / SNR signal quality
// Page 2: Packet counters RX / TX
// Called from loop(); only redraws every 3 s unless showTxOk is active.
void updateStatusDisplay() {
  unsigned long now = millis();

  // Show "TX OK!" for 3 s after a successful DOGE TX (0x10)
  if (showTxOk) {
    if (now - txOkTimestamp < 3000) {
      radioDogeDisplay.clearDisplay();
      radioDogeDisplay.setTextColor(SSD1306_WHITE);
      radioDogeDisplay.setTextSize(1);
      radioDogeDisplay.setCursor(20, 8);
      radioDogeDisplay.println("TX OK!");
      radioDogeDisplay.setCursor(8, 26);
      radioDogeDisplay.println("Such DOGE. Wow.");
      radioDogeDisplay.setCursor(12, 44);
      radioDogeDisplay.println("To the moon!");
      radioDogeDisplay.display();
      return;
    } else {
      showTxOk = false;
    }
  }

  if (now - lastStatusCycle < 3000) return;
  lastStatusCycle = now;

  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setTextSize(1);

  switch (statusPage) {
    case 0: {
      // ── Header: node address (large) ───────────────────────────────────
      radioDogeDisplay.setCursor(0, 0);
      radioDogeDisplay.println("RadioDoge Node");
      radioDogeDisplay.setTextSize(2);
      radioDogeDisplay.setCursor(0, 14);
      radioDogeDisplay.println(String(local.region) + "." + String(local.community) + "." + String(local.node));
      radioDogeDisplay.setTextSize(1);

      // ── Icon row (right-aligned, y=0): GW | WiFi | BLE ─────────────────
      uint8_t iconX = 82;
      if (gateway_mode) {
        radioDogeDisplay.setCursor(iconX, 0);
        radioDogeDisplay.print("GW");
        iconX += 18;
      }
      if (wifi_enabled) {
        radioDogeDisplay.setCursor(iconX, 0);
        radioDogeDisplay.print("Wi");
        iconX += 18;
      }
      if (bleDeviceConnected) {
        radioDogeDisplay.setCursor(iconX, 0);
        radioDogeDisplay.print("BT");
      }

      // ── "ADDR CONFLICT!" warning banner ───────────────────────────────
      if (addrConflict) {
        radioDogeDisplay.setCursor(0, 50);
        radioDogeDisplay.println("! ADDR CONFLICT !");
      }
      break;
    }
    case 1:
      // ── Signal quality ─────────────────────────────────────────────────
      radioDogeDisplay.setCursor(0, 0);
      radioDogeDisplay.println("Signal Quality");
      radioDogeDisplay.setCursor(0, 16);
      radioDogeDisplay.println("RSSI: " + String(rssi) + " dBm");
      radioDogeDisplay.setCursor(0, 30);
      radioDogeDisplay.println("SNR:  " + String(lastSnr) + " dB");
      // Simple RSSI bar (0–64 px wide, mapped -120..-50 dBm)
      {
        int bar = map(constrain(rssi, -120, -50), -120, -50, 0, 64);
        radioDogeDisplay.drawRect(0, 44, 64, 8, SSD1306_WHITE);
        radioDogeDisplay.fillRect(0, 44, bar, 8, SSD1306_WHITE);
      }
      break;
    case 2:
      // ── Packet counters ────────────────────────────────────────────────
      radioDogeDisplay.setCursor(0, 0);
      radioDogeDisplay.println("Packets");
      radioDogeDisplay.setCursor(0, 18);
      radioDogeDisplay.println("RX: " + String(pktRxCount));
      radioDogeDisplay.setCursor(0, 34);
      radioDogeDisplay.println("TX: " + String(pktTxCount));
      break;
  }

  radioDogeDisplay.display();
  statusPage = (statusPage + 1) % 3;
}

void DisplayLocalAddress(nodeAddress addr) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setCursor(0, 0);
  radioDogeDisplay.println("Address: " + String(addr.region) + "." + String(addr.community) + "." + String(addr.node));
  radioDogeDisplay.display();
}

void DisplayHardwareInfo(int version) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setCursor(0, 0);
  radioDogeDisplay.println("Heltec V" + String(version));
  radioDogeDisplay.setCursor(0, 20);
  radioDogeDisplay.println("RadioDoge");
  radioDogeDisplay.display();
}

void DisplayCustomStringMessage(String message, int yOffset) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setCursor(0, yOffset);
  radioDogeDisplay.println(message);
  radioDogeDisplay.display();
}

void DrawRadioDogeLogo() {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.drawXBitmap(0, 0, logo_bits, logo_width, logo_height, SSD1306_WHITE);
  radioDogeDisplay.display();
}

void DogeAnimation() {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.drawXBitmap(32, 0, doge_bits, doge_width, doge_height, SSD1306_WHITE);
  radioDogeDisplay.display();
  delay(1000);
}

void CoinAnimation() {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.drawXBitmap(32, 0, coin_bits, coin_width, coin_height, SSD1306_WHITE);
  radioDogeDisplay.display();
  delay(1000);
}

void DrawSendingCoinsImage(float amount) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.drawXBitmap(0, 0, sendCoin_bits, sendCoin_width, sendCoin_height, SSD1306_WHITE);
  radioDogeDisplay.display();
}

void DrawReceivingCoinsImage(float amount) {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.drawXBitmap(0, 0, rcvCoin_bits, rcvCoin_width, rcvCoin_height, SSD1306_WHITE);
  radioDogeDisplay.display();
}

// NVS Storage Functions for WiFi Credentials
void initNVS() {
  esp_err_t ret = nvs_flash_init();
  if (ret == ESP_ERR_NVS_NO_FREE_PAGES || ret == ESP_ERR_NVS_NEW_VERSION_FOUND) {
    ESP_ERROR_CHECK(nvs_flash_erase());
    ret = nvs_flash_init();
  }
  ESP_ERROR_CHECK(ret);
}

void saveWiFiCredentials(String ssid, String password) {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("wifi_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for WiFi credentials");
    return;
  }
  
  // Save SSID
  err = nvs_set_str(nvs_handle, "ssid", ssid.c_str());
  if (err != ESP_OK) {
    debugPrintln("Error saving SSID to NVS");
  }
  
  // Save password
  err = nvs_set_str(nvs_handle, "password", password.c_str());
  if (err != ESP_OK) {
    debugPrintln("Error saving password to NVS");
  }
  
  // Commit changes
  err = nvs_commit(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error committing WiFi credentials to NVS");
  }
  
  nvs_close(nvs_handle);
  debugPrintln("WiFi credentials saved to NVS");
}

bool loadWiFiCredentials() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  size_t required_size;
  
  err = nvs_open("wifi_config", NVS_READONLY, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("No WiFi credentials found in NVS");
    return false;
  }
  
  // Get SSID length
  err = nvs_get_str(nvs_handle, "ssid", NULL, &required_size);
  if (err != ESP_OK || required_size == 0) {
    nvs_close(nvs_handle);
    return false;
  }
  
  // Read SSID
  char ssid_buffer[required_size];
  err = nvs_get_str(nvs_handle, "ssid", ssid_buffer, &required_size);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  internet_ssid = String(ssid_buffer);
  
  // Get password length
  err = nvs_get_str(nvs_handle, "password", NULL, &required_size);
  if (err != ESP_OK || required_size == 0) {
    nvs_close(nvs_handle);
    return false;
  }
  
  // Read password
  char password_buffer[required_size];
  err = nvs_get_str(nvs_handle, "password", password_buffer, &required_size);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  internet_password = String(password_buffer);
  
  nvs_close(nvs_handle);
  debugPrintln("WiFi credentials loaded from NVS: " + internet_ssid);
  return true;
}

void clearWiFiCredentials() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("wifi_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for clearing WiFi credentials");
    return;
  }
  
  // Erase all keys in the namespace
  err = nvs_erase_all(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error erasing WiFi credentials from NVS");
  } else {
    debugPrintln("WiFi credentials cleared from NVS");
  }
  
  nvs_close(nvs_handle);
  
  // Clear from memory
  internet_ssid = "";
  internet_password = "";
  internet_connected = false;
  dual_wifi_mode = false;
}

// LoRa Configuration Storage Functions
void saveLoRaConfiguration(uint8_t region, uint8_t community, uint8_t node) {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("lora_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for LoRa configuration");
    return;
  }
  
  // Save region
  err = nvs_set_u8(nvs_handle, "region", region);
  if (err != ESP_OK) {
    debugPrintln("Error saving region to NVS");
  }
  
  // Save community
  err = nvs_set_u8(nvs_handle, "community", community);
  if (err != ESP_OK) {
    debugPrintln("Error saving community to NVS");
  }
  
  // Save node
  err = nvs_set_u8(nvs_handle, "node", node);
  if (err != ESP_OK) {
    debugPrintln("Error saving node to NVS");
  }
  
  // Commit changes
  err = nvs_commit(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error committing LoRa configuration to NVS");
  }
  
  nvs_close(nvs_handle);
  debugPrintln("LoRa configuration saved to NVS: " + String(region) + "." + String(community) + "." + String(node));
}

// OPTIMIZED FOR DESKTOP v0.3.3 – SAFE
// Saves node address to NVS without Serial.println() output.
// Use this instead of saveLoRaConfiguration() inside HostSerialRead() context
// where Serial.println() would pollute the binary protocol stream.
void saveLoRaConfigurationQuiet(uint8_t region, uint8_t community, uint8_t node) {
  nvs_handle_t nvs_handle;
  if (nvs_open("lora_config", NVS_READWRITE, &nvs_handle) != ESP_OK) return;
  nvs_set_u8(nvs_handle, "region", region);
  nvs_set_u8(nvs_handle, "community", community);
  nvs_set_u8(nvs_handle, "node", node);
  nvs_commit(nvs_handle);
  nvs_close(nvs_handle);
}

// v0.4.1 — Command bytes handled by HandleDesktopCommand, i.e. the app/CLI
// protocol rather than the firmware's legacy serial enum. Both the USB and the
// BLE dispatch consult this, so the two transports accept exactly the same set.
bool isDesktopCommandByte(uint8_t cmdByte) {
  return cmdByte == 0x10 || cmdByte == 0x11 || cmdByte == 0x20 || cmdByte == 0x21
      || cmdByte == 0x22 || cmdByte == 0x23 || cmdByte == 0x24 || cmdByte == 0x26
      || cmdByte == 0x27 || cmdByte == 0x28;
}

// Total packet length for a desktop command sent host -> board, or 0 when the
// command has no fixed size (0x10 DOGE_TX and 0x11 REQUEST_BALANCE carry
// variable payloads). Used only by the BLE framer, which — unlike the serial
// path — has no inter-packet delay to lean on.
uint8_t desktopCommandLength(uint8_t cmdByte) {
  switch (cmdByte) {
    case 0x20: case 0x22: case 0x26: case 0x27: return 8;   // header only
    case 0x23: case 0x24: case 0x28:            return 9;   // header + 1 flag byte
    case 0x21:                                  return 16;  // header + 8 param bytes
    default:                                    return 0;   // variable length
  }
}

// v0.4.1 — Is this board acting as an internet gateway right now?
//
// `gateway_mode` used to be a reporting flag only: the transaction/broadcast
// forwarders checked whether a gateway was *configured* (gateway_type + ip) and
// never consulted the toggle, so switching gateway mode off in the app did not
// stop the board pushing other people's transactions to the internet. The
// toggle is now authoritative and every forwarding path goes through here.
//
// Saving a gateway from the web UI enables gateway_mode (see handleApiGatewaySave),
// so a board configured entirely through the web interface keeps forwarding as
// it did before this change.
bool gatewayForwardingEnabled() {
  return gateway_mode;
}

// ─── v0.4.1 — Runtime LoRa radio reconfiguration (CMD_SET_LORA_PARAMS 0x21) ───

// Push the current lora_* globals into the radio.
// Safe to call after Radio.Init(); the radio is put into standby first so the
// SX126x is not reconfigured mid-transmission.
void applyLoRaRadioConfig() {
  // Park the radio before retuning so the SX126x is not reconfigured mid-air.
  // Radio.Sleep() is used rather than Standby() because this firmware already
  // relies on Sleep() elsewhere, so it is known to exist in the Heltec
  // LoRaWan_APP build being targeted. SetChannel/SetTxConfig/SetRxConfig wake
  // the part again, and the Rx(0) below returns it to continuous receive.
  Radio.Sleep();
  Radio.SetChannel(lora_freq_hz);
  Radio.SetTxConfig(MODEM_LORA, lora_tx_power, 0, lora_bandwidth,
                    lora_spreading_factor, lora_coding_rate,
                    LORA_PREAMBLE_LENGTH, LORA_FIX_LENGTH_PAYLOAD_ON,
                    true, 0, 0, LORA_IQ_INVERSION_ON, 3000);
  Radio.SetRxConfig(MODEM_LORA, lora_bandwidth, lora_spreading_factor,
                    lora_coding_rate, 0, LORA_PREAMBLE_LENGTH,
                    LORA_SYMBOL_TIMEOUT, LORA_FIX_LENGTH_PAYLOAD_ON,
                    0, true, 0, 0, LORA_IQ_INVERSION_ON, true);
  Radio.Rx(0);  // back to continuous receive
  isLoRaIdle = true;
}

// Reject anything the radio cannot do, or that would put the board somewhere it
// can never be reached again. Applying an out-of-range value would strand the
// board off-channel with no way back except a reflash, so validation happens
// BEFORE anything is written or applied.
//   sf     7..12
//   bw     0 (125 kHz), 1 (250 kHz), 2 (500 kHz)
//   cr     1..4  (4/5 .. 4/8)
//   freq   150 MHz .. 960 MHz — spans the LoRa sub-GHz bands (433/470/868/915)
//   power  2..22 dBm
bool loRaParamsAreValid(uint32_t freq_hz, uint8_t sf, uint8_t bw, uint8_t cr, int8_t power) {
  if (sf < 7 || sf > 12) return false;
  if (bw > 2) return false;
  if (cr < 1 || cr > 4) return false;
  if (power < 2 || power > 22) return false;
  if (freq_hz < 150000000UL || freq_hz > 960000000UL) return false;
  return true;
}

void saveLoRaRadioParamsQuiet() {
  nvs_handle_t h;
  if (nvs_open("rd_settings", NVS_READWRITE, &h) != ESP_OK) return;
  nvs_set_u32(h, "lora_freq", lora_freq_hz);
  nvs_set_u8(h, "lora_sf", lora_spreading_factor);
  nvs_set_u8(h, "lora_bw", lora_bandwidth);
  nvs_set_u8(h, "lora_cr", lora_coding_rate);
  nvs_set_i8(h, "lora_pwr", lora_tx_power);
  nvs_commit(h);
  nvs_close(h);
}

// Returns true if a stored, still-valid set was loaded into the globals.
// A stored set that fails validation (e.g. written by a different firmware
// build) is ignored so the board falls back to its compile-time defaults
// rather than booting onto an unusable channel.
bool loadLoRaRadioParams() {
  nvs_handle_t h;
  if (nvs_open("rd_settings", NVS_READONLY, &h) != ESP_OK) return false;
  uint32_t freq = lora_freq_hz;
  uint8_t sf = lora_spreading_factor, bw = lora_bandwidth, cr = lora_coding_rate;
  int8_t pwr = lora_tx_power;
  bool any = false;
  if (nvs_get_u32(h, "lora_freq", &freq) == ESP_OK) any = true;
  if (nvs_get_u8(h, "lora_sf", &sf)     == ESP_OK) any = true;
  if (nvs_get_u8(h, "lora_bw", &bw)     == ESP_OK) any = true;
  if (nvs_get_u8(h, "lora_cr", &cr)     == ESP_OK) any = true;
  if (nvs_get_i8(h, "lora_pwr", &pwr)   == ESP_OK) any = true;
  nvs_close(h);
  if (!any) return false;
  if (!loRaParamsAreValid(freq, sf, bw, cr, pwr)) return false;
  lora_freq_hz = freq;
  lora_spreading_factor = sf;
  lora_bandwidth = bw;
  lora_coding_rate = cr;
  lora_tx_power = pwr;
  return true;
}

// v0.3.6 — gateway_mode NVS persistence
// Saves silently (no Serial.println) — safe inside binary protocol handlers.
void saveGatewayModeQuiet(bool mode) {
  nvs_handle_t h;
  if (nvs_open("rd_settings", NVS_READWRITE, &h) != ESP_OK) return;
  nvs_set_u8(h, "gw_mode", mode ? 1 : 0);
  nvs_commit(h);
  nvs_close(h);
}

bool loadGatewayMode() {
  nvs_handle_t h;
  uint8_t val = 0;
  if (nvs_open("rd_settings", NVS_READONLY, &h) != ESP_OK) return false;
  esp_err_t err = nvs_get_u8(h, "gw_mode", &val);
  nvs_close(h);
  if (err != ESP_OK) return false;
  gateway_mode = (val != 0);
  return true;
}

// v0.3.7 — WiFi enabled NVS persistence
void saveWifiEnabledQuiet(bool enabled) {
  nvs_handle_t h;
  if (nvs_open("rd_settings", NVS_READWRITE, &h) != ESP_OK) return;
  nvs_set_u8(h, "wifi_on", enabled ? 1 : 0);
  nvs_commit(h);
  nvs_close(h);
}

bool loadWifiEnabled() {
  nvs_handle_t h;
  uint8_t val = 1; // default: wifi on
  if (nvs_open("rd_settings", NVS_READONLY, &h) != ESP_OK) return false;
  esp_err_t err = nvs_get_u8(h, "wifi_on", &val);
  nvs_close(h);
  if (err != ESP_OK) return false; // key not set yet; keep default
  wifi_enabled = (val != 0);
  return true;
}

// v0.4.0 — BLE advertising NVS persistence (CMD_BLE_TOGGLE 0x28)
void saveBleEnabledQuiet(bool enabled) {
  nvs_handle_t h;
  if (nvs_open("rd_settings", NVS_READWRITE, &h) != ESP_OK) return;
  nvs_set_u8(h, "ble_on", enabled ? 1 : 0);
  nvs_commit(h);
  nvs_close(h);
}

bool loadBleEnabled() {
  nvs_handle_t h;
  uint8_t val = 1; // default: BLE on
  if (nvs_open("rd_settings", NVS_READONLY, &h) != ESP_OK) return false;
  esp_err_t err = nvs_get_u8(h, "ble_on", &val);
  nvs_close(h);
  if (err != ESP_OK) return false; // key not set yet; keep default
  ble_enabled = (val != 0);
  return true;
}

bool loadLoRaConfiguration() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  uint8_t region, community, node;
  
  err = nvs_open("lora_config", NVS_READONLY, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("No LoRa configuration found in NVS");
    return false;
  }
  
  // Read region
  err = nvs_get_u8(nvs_handle, "region", &region);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  
  // Read community
  err = nvs_get_u8(nvs_handle, "community", &community);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  
  // Read node
  err = nvs_get_u8(nvs_handle, "node", &node);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  
  nvs_close(nvs_handle);
  
  // Set the loaded configuration
  local.region = region;
  local.community = community;
  local.node = node;
  
  debugPrintln("LoRa configuration loaded from NVS: " + String(region) + "." + String(community) + "." + String(node));
  return true;
}

void clearLoRaConfiguration() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("lora_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for clearing LoRa configuration");
    return;
  }
  
  // Erase all keys in the namespace
  err = nvs_erase_all(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error erasing LoRa configuration from NVS");
  } else {
    debugPrintln("LoRa configuration cleared from NVS");
  }
  
  nvs_close(nvs_handle);
  
  // Reset to default values
  local.region = 10;
  local.community = 1;
  local.node = 1;
}

// AP Password Management Functions
void saveAPPassword(String password) {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("ap_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for AP password");
    return;
  }
  
  // Save password
  err = nvs_set_str(nvs_handle, "password", password.c_str());
  if (err != ESP_OK) {
    debugPrintln("Error saving AP password to NVS");
  }
  
  // Commit changes
  err = nvs_commit(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error committing AP password to NVS");
  }
  
  nvs_close(nvs_handle);
  debugPrintln("AP password saved to NVS");
}

bool loadAPPassword() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  size_t required_size;
  
  err = nvs_open("ap_config", NVS_READONLY, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("No AP password found in NVS, using default");
    return false;
  }
  
  // Get password length
  err = nvs_get_str(nvs_handle, "password", NULL, &required_size);
  if (err != ESP_OK || required_size == 0) {
    nvs_close(nvs_handle);
    return false;
  }
  
  // Read password
  char password_buffer[required_size];
  err = nvs_get_str(nvs_handle, "password", password_buffer, &required_size);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  
  ap_password = String(password_buffer);
  nvs_close(nvs_handle);
  debugPrintln("AP password loaded from NVS");
  return true;
}

void clearAPPassword() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("ap_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for clearing AP password");
    return;
  }
  
  // Erase all keys in the namespace
  err = nvs_erase_all(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error erasing AP password from NVS");
  } else {
    debugPrintln("AP password cleared from NVS");
  }
  
  nvs_close(nvs_handle);
  
  // Reset to default
  ap_password = "radiodoge";
}

// Gateway Credential Management Functions
// v0.4.2 — NVS key names are capped at 15 usable characters
// (NVS_KEY_NAME_MAX_SIZE is 16 *including* the NUL). The old key name was 16,
// so every nvs_set_str/nvs_get_str with it returned ESP_ERR_NVS_KEY_TOO_LONG:
// the endpoint was never stored and never loaded, while /api/gateway/save still
// reported success. A board configured with a custom endpoint path silently
// forgot it on every reboot and POSTed transactions to the bare gateway URL.
// Renamed to "gw_endpoint" (11), matching the abbreviated style already used by
// "gw_mode" and "gw_pass".
#define NVS_KEY_GATEWAY_ENDPOINT "gw_endpoint"

void saveGatewayCredentials(String type, String ip, String port, String endpoint, String username, String password) {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("gateway_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for gateway credentials: " + String(err));
    addLog("ERROR: Failed to open NVS handle for gateway save - Error code: " + String(err));
    return;
  }
  
  // Clear any existing password keys to avoid conflicts
  nvs_erase_key(nvs_handle, "gw_pass");
  nvs_erase_key(nvs_handle, "gateway_pass");
  nvs_erase_key(nvs_handle, "gateway_password");
  
  // Save gateway type
  err = nvs_set_str(nvs_handle, "gateway_type", type.c_str());
  if (err != ESP_OK) {
    debugPrintln("Error saving gateway type to NVS");
  }
  
  // Save IP
  err = nvs_set_str(nvs_handle, "gateway_ip", ip.c_str());
  if (err != ESP_OK) {
    debugPrintln("Error saving gateway IP to NVS");
  }
  
  // Save port
  err = nvs_set_str(nvs_handle, "gateway_port", port.c_str());
  if (err != ESP_OK) {
    debugPrintln("Error saving gateway port to NVS");
  }
  
  // Save endpoint
  err = nvs_set_str(nvs_handle, NVS_KEY_GATEWAY_ENDPOINT, endpoint.c_str());
  if (err != ESP_OK) {
    debugPrintln("Error saving gateway endpoint to NVS");
  }
  
  // Save username (if provided)
  if (username.length() > 0) {
    err = nvs_set_str(nvs_handle, "gateway_user", username.c_str());
    if (err != ESP_OK) {
      debugPrintln("Error saving gateway username to NVS");
    } else {
      debugPrintln("Gateway username saved: [REDACTED]");
    }
  }
  
  // Save password (if provided)
  if (password.length() > 0) {
    debugPrintln("Saving gateway password: [REDACTED]");
    
    // Use shorter key name (ESP32 NVS max key length is 15 characters)
    err = nvs_set_str(nvs_handle, "gw_pass", password.c_str());
    if (err != ESP_OK) {
      debugPrintln("Error saving gateway password to NVS: " + String(err));
      addLog("ERROR: Failed to save gateway password to NVS - Error code: " + String(err));
    } else {
      debugPrintln("Gateway password saved: [HIDDEN]");
    }
  } else {
    debugPrintln("No password provided for saving");
    addLog("WARNING: No password provided for gateway save");
  }
  
  // Commit changes
  err = nvs_commit(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error committing gateway credentials to NVS: " + String(err));
    addLog("ERROR: Failed to save gateway credentials to NVS");
  } else {
    debugPrintln("Gateway credentials saved to NVS");
    addLog("Gateway credentials saved: " + type + " at " + ip + ":" + port);
  }
  
  nvs_close(nvs_handle);
}

bool loadGatewayCredentials() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("gateway_config", NVS_READONLY, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("No gateway credentials found in NVS");
    return false;
  }
  
  // Load gateway type
  size_t type_len = 16;
  char type_buffer[16];
  err = nvs_get_str(nvs_handle, "gateway_type", type_buffer, &type_len);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  gateway_type = String(type_buffer);
  
  // Load IP
  size_t ip_len = 16;
  char ip_buffer[16];
  err = nvs_get_str(nvs_handle, "gateway_ip", ip_buffer, &ip_len);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  gateway_ip = String(ip_buffer);
  
  // Load port
  size_t port_len = 8;
  char port_buffer[8];
  err = nvs_get_str(nvs_handle, "gateway_port", port_buffer, &port_len);
  if (err != ESP_OK) {
    nvs_close(nvs_handle);
    return false;
  }
  gateway_port = String(port_buffer);
  
  // Load endpoint (optional)
  size_t endpoint_len = 64;
  char endpoint_buffer[64];
  err = nvs_get_str(nvs_handle, NVS_KEY_GATEWAY_ENDPOINT, endpoint_buffer, &endpoint_len);
  if (err == ESP_OK) {
    gateway_endpoint = String(endpoint_buffer);
  } else {
    gateway_endpoint = "";
    debugPrintln("No gateway endpoint found");
  }
  
  // Load username (optional)
  size_t username_len = 128;
  char username_buffer[128];
  err = nvs_get_str(nvs_handle, "gateway_user", username_buffer, &username_len);
  if (err == ESP_OK) {
    gateway_username = String(username_buffer);
    debugPrintln("Gateway username loaded: [REDACTED]");
  } else {
    debugPrintln("No gateway username found");
  }
  
  // Load password (optional) - try multiple key names
  size_t password_len = 256;  // Increased buffer size
  char password_buffer[256];
  
  // First try the working key name
  err = nvs_get_str(nvs_handle, "gw_pass", password_buffer, &password_len);
  if (err != ESP_OK) {
    // Try old key name for backward compatibility
    password_len = 256;
    err = nvs_get_str(nvs_handle, "gateway_pass", password_buffer, &password_len);
    if (err != ESP_OK) {
      // Try the long key name (in case it was saved before the fix)
      password_len = 256;
      err = nvs_get_str(nvs_handle, "gateway_password", password_buffer, &password_len);
    }
  }
  
  if (err == ESP_OK) {
    gateway_password = String(password_buffer);
    debugPrintln("Loaded gateway password: [HIDDEN]");
    debugPrintln("Gateway password loaded: [REDACTED]");
  } else {
    debugPrintln("No gateway password found: " + String(err));
    addLog("WARNING: No gateway password found in NVS");
    gateway_password = "";
  }
  
  nvs_close(nvs_handle);
  debugPrintln("Gateway credentials loaded from NVS");
  addLog("Gateway credentials loaded: " + gateway_type + " at " + gateway_ip + ":" + gateway_port);
  return true;
}

void clearGatewayCredentials() {
  nvs_handle_t nvs_handle;
  esp_err_t err;
  
  err = nvs_open("gateway_config", NVS_READWRITE, &nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error opening NVS handle for clearing gateway credentials");
    return;
  }
  
  // Erase all keys in the namespace
  err = nvs_erase_all(nvs_handle);
  if (err != ESP_OK) {
    debugPrintln("Error erasing gateway credentials from NVS");
    addLog("ERROR: Failed to clear gateway credentials from NVS");
  } else {
    debugPrintln("Gateway credentials cleared from NVS");
    addLog("Gateway credentials cleared from NVS");
  }
  
  nvs_close(nvs_handle);
}

// Function to escape JSON strings
String escapeJsonString(String input) {
  input.replace("\\", "\\\\");
  input.replace("\"", "\\\"");
  input.replace("\n", "\\n");
  input.replace("\r", "\\r");
  input.replace("\t", "\\t");
  input.replace("\b", "\\b");
  input.replace("\f", "\\f");
  // Remove any other control characters that might break JSON
  for (int i = 0; i < input.length(); i++) {
    if (input.charAt(i) < 32 && input.charAt(i) != '\n' && input.charAt(i) != '\r' && input.charAt(i) != '\t') {
      input.setCharAt(i, '?');
    }
  }
  return input;
}

// Logging functions
// v0.4.2 — Runtime diagnostics go to the log ring buffer, never to Serial.
//
// The host speaks a binary protocol on that same wire. Free-form text written
// while a host is connected is not merely untidy: the command set overlaps
// printable ASCII, so a log line can be mistaken for a packet header, and the
// framer will then consume the real packet queued behind it. That is why the
// host needs a resynchronisation heuristic at all.
//
// Anything a developer wants to watch is already in the web UI's log view and in
// `GET /api/logs`. The banner printed by setup() is deliberately left on Serial:
// it happens once, before a host is talking, and it is how you tell a freshly
// flashed board is alive.
void debugPrintln(String message) {
  addLog(message);
}

void addLog(String message) {
  String timestamp = String(millis());
  String logEntry = "[" + timestamp + "] " + message;
  
  // Truncate if too long
  if (logEntry.length() > MAX_LOG_LENGTH) {
    logEntry = logEntry.substring(0, MAX_LOG_LENGTH - 3) + "...";
  }
  
  logBuffer[logIndex] = logEntry;
  logIndex = (logIndex + 1) % MAX_LOG_ENTRIES;
  if (logCount < MAX_LOG_ENTRIES) {
    logCount++;
  }

  // v0.4.2 — This used to end with `Serial.println(logEntry)`, which is where
  // essentially all of the protocol noise came from: every one of the hundred-odd
  // addLog() calls — several of them fired per received packet, in the middle of
  // handling it — wrote a line of text down the wire the host reads packets from.
  //
  // That matters more than untidiness. The desktop command set overlaps printable
  // ASCII (0x20 is both CMD_GET_FIRMWARE_VERSION and the space character), so the
  // host's framer can mistake a log line for a packet header and then consume the
  // real packet queued behind it. Resynchronisation exists to recover from that,
  // but the best outcome is for it never to happen while a transaction is in
  // flight.
  //
  // The log is still fully available: MAX_LOG_ENTRIES of it in the web UI and at
  // GET /api/logs. Set HOST_SERIAL_DEBUG to 1 to put it back on the wire when
  // debugging with a plain serial monitor and no host software attached.
#if HOST_SERIAL_DEBUG
  Serial.println(logEntry);
#endif
}

void addDisplayLog(String message) {
  addLog("[DISPLAY] " + message);
}

bool validatePassword(String password) {
  // Check length
  if (password.length() < MIN_PASSWORD_LENGTH || password.length() > MAX_PASSWORD_LENGTH) {
    return false;
  }
  
  // Check for at least one letter and one number
  bool hasLetter = false;
  bool hasNumber = false;
  
  for (int i = 0; i < password.length(); i++) {
    char c = password.charAt(i);
    if (isAlpha(c)) {
      hasLetter = true;
    } else if (isDigit(c)) {
      hasNumber = true;
    }
  }
  
  return hasLetter && hasNumber;
}

void restartAP() {
  debugPrintln("Restarting Access Point with new password...");
  
  // Stop current AP
  WiFi.softAPdisconnect(true);
  delay(1000);
  
  // Start AP with new password
  WiFi.softAP(ap_ssid, ap_password.c_str());
  debugPrintln("Access Point restarted");
  debugPrintln("SSID: " + String(ap_ssid));
  debugPrintln("IP address: " + WiFi.softAPIP().toString());
}

// WiFi Management Functions
void setupDualWiFi() {
  // Load stored WiFi credentials
  if (loadWiFiCredentials()) {
    debugPrintln("Found stored WiFi credentials, attempting connection...");
  }
  
  // Start Access Point
  WiFi.softAP(ap_ssid, ap_password.c_str());
  debugPrintln("WiFi AP started");
  debugPrintln("AP SSID: " + String(ap_ssid));
  debugPrintln("AP IP address: " + WiFi.softAPIP().toString());
  addLog("[WiFi] Access Point started - SSID: " + String(ap_ssid) + ", IP: " + WiFi.softAPIP().toString());
  
  // Try to connect to internet WiFi if credentials are available
  if (internet_ssid.length() > 0) {
    connectToInternetWiFi();
  }
  
  // Setup internet bridging if both AP and internet are available
  if (internet_connected) {
    setupInternetBridge();
  }
  
  // Display WiFi status
  DisplayWiFiStatus();
}

void connectToInternetWiFi() {
  if (internet_ssid.length() == 0) return;
  
  debugPrintln("Attempting to connect to internet WiFi...");
  DisplayCustomStringMessage("Connecting to WiFi...", 0);
  addLog("Attempting to connect to internet WiFi: " + internet_ssid);
  
  WiFi.begin(internet_ssid.c_str(), internet_password.c_str());
  
  int attempts = 0;
  while (WiFi.status() != WL_CONNECTED && attempts < 20) {
    delay(500);
    // Progress was a dot per attempt on the wire the host protocol uses.
    attempts++;
  }
  
  if (WiFi.status() == WL_CONNECTED) {
    internet_connected = true;
    dual_wifi_mode = true;
    // (progress separator; diagnostics go to the log)
    debugPrintln("Internet WiFi connected!");
    debugPrintln("Internet IP address: " + WiFi.localIP().toString());
    DisplayCustomStringMessage("Internet Connected!", 0);
    addLog("Internet WiFi connected! IP: " + WiFi.localIP().toString());
    
    // Save credentials to NVS for future boots
    saveWiFiCredentials(internet_ssid, internet_password);
  } else {
    internet_connected = false;
    dual_wifi_mode = false;
    // (progress separator; diagnostics go to the log)
    debugPrintln("Failed to connect to internet WiFi");
    DisplayCustomStringMessage("No Internet", 0);
    addLog("Failed to connect to internet WiFi: " + internet_ssid);
  }
}

void disconnectInternetWiFi() {
  if (internet_connected) {
    WiFi.disconnect();
    internet_connected = false;
    dual_wifi_mode = false;
    debugPrintln("Disconnected from internet WiFi");
    DisplayCustomStringMessage("Internet Disconnected", 0);
  }
}

void DisplayWiFiStatus() {
  radioDogeDisplay.clearDisplay();
  radioDogeDisplay.setTextSize(1);
  radioDogeDisplay.setTextColor(SSD1306_WHITE);
  radioDogeDisplay.setCursor(0, 0);
  radioDogeDisplay.println("WiFi Status:");
  radioDogeDisplay.setCursor(0, 15);
  radioDogeDisplay.println("AP: " + String(ap_ssid));
  radioDogeDisplay.setCursor(0, 30);
  if (internet_connected) {
    radioDogeDisplay.println("Internet: " + internet_ssid);
    radioDogeDisplay.setCursor(0, 45);
    radioDogeDisplay.println("IP: " + WiFi.localIP().toString());
  } else if (internet_ssid.length() > 0) {
    radioDogeDisplay.println("Internet: " + internet_ssid);
    radioDogeDisplay.setCursor(0, 45);
    radioDogeDisplay.println("(Stored, not connected)");
  } else {
    radioDogeDisplay.println("Internet: Not Configured");
  }
  radioDogeDisplay.display();
}

// Internet Bridging Functions
void setupInternetBridge() {
  if (!internet_connected) {
    debugPrintln("Cannot setup internet bridge - no internet connection");
    return;
  }
  
  debugPrintln("Setting up internet bridge...");
  addLog("[WiFi] Setting up internet bridge between AP and internet WiFi");
  
  // Configure AP with proper gateway and subnet
  WiFi.softAPConfig(ap_gateway, ap_gateway, ap_subnet);
  
  // Start DNS server to handle all DNS requests
  dnsServer.start(53, "*", ap_gateway);
  
  // Initialize client tracking
  clientCount = 0;
  for (int i = 0; i < 8; i++) {
    connectedClients[i].active = false;
    connectedClients[i].lastSeen = 0;
  }
  
  // Enable internet bridging
  internet_bridge_enabled = true;
  
  debugPrintln("Internet bridge enabled");
  debugPrintln("AP Gateway: " + ap_gateway.toString());
  debugPrintln("AP Subnet: " + ap_subnet.toString());
  debugPrintln("Internet IP: " + WiFi.localIP().toString());
  debugPrintln("DNS Server: Running on port 53");
  addLog("[WiFi] Internet bridge enabled - AP clients can now access internet");
  addLog("[WiFi] DNS Server running on 192.168.4.1:53");
}

void handleInternetBridge() {
  if (internet_bridge_enabled) {
    // Handle DNS requests
    dnsServer.processNextRequest();
    
    // Check for connected clients periodically
    if (millis() - lastClientCheck > 5000) { // Every 5 seconds
      checkConnectedClients();
      lastClientCheck = millis();
    }
    
    // Note: ESP32 has limited NAT capabilities
    // For full internet bridging, you may need to:
    // 1. Use a different approach like HTTP proxy
    // 2. Configure your router to allow the ESP32 to act as a bridge
    // 3. Use a more powerful device for true NAT functionality
  }
}

void checkConnectedClients() {
  // Get list of connected stations
  int stationCount = WiFi.softAPgetStationNum();
  
  if (stationCount != clientCount) {
    clientCount = stationCount;
    addLog("[WiFi] Connected clients: " + String(clientCount));
    
    // Note: ESP32 WiFi library doesn't provide direct access to client IPs
    // We can only track the count of connected stations
    // For actual IP tracking, we would need to implement DHCP server logging
    // or use a different approach like monitoring ARP requests
    
    if (stationCount > 0) {
      addLog("[WiFi] " + String(stationCount) + " client(s) connected to RadioDoge AP");
    } else {
      addLog("[WiFi] No clients connected to RadioDoge AP");
    }
  }
}

void enableInternetBridge() {
  if (internet_connected && !internet_bridge_enabled) {
    setupInternetBridge();
    addLog("[WiFi] Internet bridge enabled via API");
  } else if (!internet_connected) {
    addLog("[WiFi] Cannot enable internet bridge - no internet connection");
  } else {
    addLog("[WiFi] Internet bridge already enabled");
  }
}

void disableInternetBridge() {
  if (internet_bridge_enabled) {
    dnsServer.stop();
    internet_bridge_enabled = false;
    addLog("[WiFi] Internet bridge disabled");
  } else {
    addLog("[WiFi] Internet bridge was not enabled");
  }
}

// Internet Gateway Functions
String sendTransactionToInternet(String transactionData) {
  if (!internet_connected) {
    debugPrintln("No internet connection available");
    return "{\"error\":\"No internet connection available\"}";
  }
  
  HTTPClient http;
  http.begin("https://api.blockcypher.com/v1/doge/main/txs/push");
  http.addHeader("Content-Type", "application/json");
  
  String jsonPayload = "{\"tx\":\"" + transactionData + "\"}";
  
  int httpResponseCode = http.POST(jsonPayload);
  String response = "";
  
  if (httpResponseCode > 0) {
    response = http.getString();
    debugPrintln("Transaction sent to internet gateway");
    debugPrintln("Response: " + response);
    addLog("Transaction sent to internet gateway - Response: " + response);
  } else {
    response = "{\"error\":\"HTTP Error " + String(httpResponseCode) + "\"}";
    debugPrintln("Error sending transaction to internet: " + String(httpResponseCode));
  }
  
  http.end();
  return response;
}

String sendTransactionToCustomGateway(String transactionData, String gatewayUrl) {
  if (!internet_connected) {
    debugPrintln("No internet connection available");
    return "{\"error\":\"No internet connection available\"}";
  }
  
  HTTPClient http;
  http.begin(gatewayUrl);
  http.addHeader("Content-Type", "application/json");
  
  String jsonPayload = "{\"tx\":\"" + transactionData + "\"}";
  
  int httpResponseCode = http.POST(jsonPayload);
  String response = "";
  
  if (httpResponseCode > 0) {
    response = http.getString();
    debugPrintln("Transaction sent to custom gateway");
    debugPrintln("Response: " + response);
  } else {
    response = "{\"error\":\"HTTP Error " + String(httpResponseCode) + "\"}";
    debugPrintln("Error sending transaction to custom gateway: " + String(httpResponseCode));
  }
  
  http.end();
  return response;
}

// Older test that allows for the sending of messages (specified from the host over serial) between devices without any addressing
void RawSerialMessageSendAndReceive() {
  if (isLoRaIdle) {
    isLoRaIdle = false;
    // Indicating that we are moving into rx mode (debug)
    addLog("[LoRa] Detected idle state - switching to RX mode");
    // Enter back into RX mode
    Radio.Rx(0);
  }

  // Check if there is data on the serial port to send out
  if (CreateMessage()) {
    isLoRaIdle = false;
    // Indicating that we are starting a TX
    //Serial.println("VERY TX");
    //Serial.printf("\r\nSending packet \"%X,\" , Length %d\r\n", txPacket, strlen(txPacket));
    Radio.Send((uint8_t *)txPacket, strlen(txPacket));
  }

  Radio.IrqProcess();
}

// Updated core functionality supporting addressing, pings, acks, and user defined messages
void CommandAndControlLoop() {
  if (needToSendACK) {
    // For now we will delay before sending an ACK (debug purposes)
    delay(2000);
    isLoRaIdle = false;
    needToSendACK = false;
    SendACK(dest);
  }

  if (isLoRaIdle) {
    isLoRaIdle = false;
    // Indicating that we are moving into rx mode (debug)
    addLog("[LoRa] Detected idle state - switching to RX mode");
    // Enter back into RX mode
    Radio.Rx(0);
  }

  // Check if there is data on the serial port to send out
  if (Serial.available() > 0) {
    HostSerialRead();
  }

  Radio.IrqProcess();
}

void OnTxDone(void) {
  // v0.3.6 — pktTxCount tracked for OLED status display
  pktTxCount++;
  loRaTxPending = false;
  addLog("[LoRa] TX completed successfully");
  isLoRaIdle = true;
}

void OnTxTimeout(void) {
  Radio.Sleep();
  // Indicate that TX failed (debug)
  //Serial.println("OOPS TX BAD");
  loRaTxPending = false;
  addLog("[LoRa] TX timeout - transmission failed");
  isLoRaIdle = true;
}

// v0.4.2 — Transmit a packet and block until the radio has actually finished.
//
// Radio.Send() only *starts* a transmission; completion arrives asynchronously
// as TxDone via Radio.IrqProcess(). Callers that returned immediately after
// Send() left the main loop free to flip the radio back into receive mid-packet
// (see loRaTxPending), and callers that sent several packets in a row — a
// multipart sequence — overwrote each transmission with the next one.
//
// Pumping IrqProcess() here is what makes a multipart send work: each part is
// fully on the air before the next byte of the next part is read from serial,
// and the host acknowledgement written afterwards doubles as flow control, so
// the host paces itself to real airtime instead of a guessed delay.
//
// Returns true if TxDone was seen, false on TxTimeout or if the radio never
// reported completion within LORA_TX_TIMEOUT_MS.
bool SendLoRaAndWait(uint8_t *buffer, uint8_t length) {
  // Never start a transmission on top of one still in progress.
  uint32_t waitStart = millis();
  while (loRaTxPending && (millis() - waitStart) < LORA_TX_TIMEOUT_MS) {
    Radio.IrqProcess();
    delay(1);
  }

  loRaTxPending = true;
  isLoRaIdle = false;
  Radio.Send(buffer, length);

  uint32_t start = millis();
  while (loRaTxPending && (millis() - start) < LORA_TX_TIMEOUT_MS) {
    Radio.IrqProcess();
    delay(1);
  }

  if (loRaTxPending) {
    // The radio never reported completion. Clear the flag so one stuck
    // transmission cannot wedge every later send, and let the main loop put the
    // radio back into receive.
    loRaTxPending = false;
    isLoRaIdle = true;
    addLog("[LoRa] TX did not complete within " + String(LORA_TX_TIMEOUT_MS) + " ms");
    return false;
  }
  return true;
}

void OnRxTimeout(void) {
  Radio.Sleep();
  // Indicate that RX failed (debug)
  // Probably should not see this since RX should not timeout
  //Serial.println("OOPS RX BAD");
  addLog("[LoRa] RX timeout - no packet received");
  isLoRaIdle = true;
}

// Callback function run when a packet is received by the LoRa module
void OnRxDone(uint8_t *payload, uint16_t messageSize, int16_t rssiMeasured, int8_t snr) {
  rssi = rssiMeasured;
  lastSnr = snr;   // v0.3.6 — track for OLED status cycle
  pktRxCount++;    // v0.3.6 — track for OLED status cycle
  // v0.4.2 — Clamp before copying. `messageSize` comes from the radio driver and
  // everything downstream indexes rxPacket with it; the NUL terminator alone
  // needs one byte more than the payload. This is the single entry point for
  // every byte an unauthenticated sender can put into this device, so it is
  // worth being certain here rather than in each of its readers.
  if (messageSize > BUFFER_SIZE - 1) {
    addLog("[LoRa] Oversized RX packet (" + String(messageSize) + " bytes) - truncating");
    messageSize = BUFFER_SIZE - 1;
  }
  rxSize = (int16_t)messageSize;
  memcpy(rxPacket, payload, messageSize);
  rxPacket[messageSize] = '\0';
  Radio.Sleep();
  addLog("[LoRa] RX packet received - RSSI: " + String(rssi) + " dBm, Length: " + String(messageSize) + " bytes, SNR: " + String(snr) + " dB");

  // v0.3.7 — Duplicate node address detection
  // Bytes [2..4] in a standard LoRa packet are source region.community.node
  if (messageSize >= 5 &&
      rxPacket[2] == local.region &&
      rxPacket[3] == local.community &&
      rxPacket[4] == local.node) {
    addrConflict = true;
    addLog("[WARN] Duplicate node address detected! Another node is using " +
           String(local.region) + "." + String(local.community) + "." + String(local.node));
    // Notify the host over serial: CMD_ADDR_CONFLICT (0x25).
    //
    // v0.4.2 — this used to go out in the legacy [cmd, payloadSize, payload…]
    // shape, five bytes long, but the host frames 0x25 as a standard 8-byte
    // desktop packet. It therefore waited for three more bytes and took them
    // from whatever arrived next, so a duplicate address on the mesh corrupted
    // the packet behind the warning. The conflicting address is this board's
    // own, which the header already carries as the source.
    uint8_t conflictMsg[8] = {0x25, 0x00, local.region, local.community, local.node,
                              0xFF, 0xFF, 0xFF};
    Serial.write(conflictMsg, 8);
  }

  ParseReceivedMessage();
  isLoRaIdle = true;
}

// Create a custom message that is specified by the host over the serial port
bool CreateMessage() {
  if (Serial.available() > 0) {
    String readString = Serial.readStringUntil('\0');
    // Echo back the read string (debug)
    //Serial.printf("REPLY: %s\n", readString);
    // Display on the screen
    DisplayTXMessage(readString, dest);
    readString.toCharArray(txPacket, BUFFER_SIZE);
    return true;
  }
  return false;
}

// Prepopulates control message buffers with local address
void InitControlMessages() {
  controlPacket[0] = (uint8_t)UNKNOWN;
  // Payload size for control packets is fixed at 6
  controlPacket[1] = 6;
  controlPacket[2] = local.region;
  controlPacket[3] = local.community;
  controlPacket[4] = local.node;
}

// Set the address of this device
void SetLocalAddressFromSerialBuffer(int offset) {
  local.region = serialBuf[offset];
  local.community = serialBuf[offset + 1];
  local.node = serialBuf[offset + 2];
}

// Set the destination address for the next transmission
void SetDestinationFromSerialBuffer(int offset) {
  dest.region = serialBuf[offset];
  dest.community = serialBuf[offset + 1];
  dest.node = serialBuf[offset + 2];
}

// Set the destination to the global broadcast / reserved address for the next transmission
void SetDestinationAsBroadcast()
{
  dest.region = 255;
  dest.community = 255;
  dest.node = 255;
}

// Extracts the address of the sending node from the received packet buffer
void SetSenderAddress()
{
  // For multipart packets, addresses are in different positions
  if (rxPacket[0] == MULTIPART_PACKET) {
    // Multipart packet: [type][dest_region][dest_community][dest_node][src_region][src_community][src_node]...
    senderAddress.region = rxPacket[4];
    senderAddress.community = rxPacket[5];
    senderAddress.node = rxPacket[6];
  } else {
    // Regular packet: [type][header][src_region][src_community][src_node][dest_region][dest_community][dest_node]...
    senderAddress.region = rxPacket[SENDER_ADDRESS_OFFSET];
    senderAddress.community = rxPacket[SENDER_ADDRESS_OFFSET + 1];
    senderAddress.node = rxPacket[SENDER_ADDRESS_OFFSET + 2];
  }
}

// Retrieve the local address
void GetLocalAddress() {
  hostCommandReply[1] = 3;
  hostCommandReply[2] = local.region;
  hostCommandReply[3] = local.community;
  hostCommandReply[4] = local.node;
  Serial.write(hostCommandReply, 5);
  //Serial.printf("MUCH LCL: %d.%d.%d\n", local.region, local.community, local.node);
}

// Indicate to the host that an ACK was received and display it on the screen
void ReceivedACK() {
  SetSenderAddress();
  DisplayRXMessage("ACK", senderAddress);
  // Notify host: CMD_RECEIVED_ACK (0x29), standard 8-byte header, src = ACK sender
  uint8_t ackMsg[8] = {0x29, 0x00,
                       senderAddress.region, senderAddress.community, senderAddress.node,
                       0xFF, 0xFF, 0xFF};
  Serial.write(ackMsg, 8);
}

// Indicate to the host that a ping was received and display it on the screen
void ReceivedPing() {
  SetSenderAddress();
  DisplayRXMessage("Ping!", senderAddress);
  // Notify host: CMD_RECEIVED_PING (0x2A), standard 8-byte header, src = ping sender
  uint8_t pingMsg[8] = {0x2A, 0x00,
                        senderAddress.region, senderAddress.community, senderAddress.node,
                        0xFF, 0xFF, 0xFF};
  Serial.write(pingMsg, 8);
}

// Send a ping to the specified destination address
void SendPing(nodeAddress destination) {
  // Message structure will by [message type, sender, destination]
  controlPacket[0] = (uint8_t)PING;
  controlPacket[5] = destination.region;
  controlPacket[6] = destination.community;
  controlPacket[7] = destination.node;
  isLoRaIdle = false;
  DisplayTXMessage("Ping", destination);
  addLog("[LoRa] Sending PING to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node));
  // v0.3.7 — ACK host BEFORE Radio.Send (LoRa TX at SF7 takes ~200-500ms; serial ACK must arrive first)
  Serial.write(hostACK, HOST_ACK_NACK_SIZE);
  SendLoRaAndWait(controlPacket, CONTROL_SIZE);
}

// Send an ACK to the specified destination address
void SendACK(nodeAddress destination) {
  // Message structure will by [message type, sender, destination]
  controlPacket[0] = (uint8_t)ACK;
  controlPacket[5] = destination.region;
  controlPacket[6] = destination.community;
  controlPacket[7] = destination.node;
  DisplayTXMessage("ACK", destination);
  isLoRaIdle = false;
  addLog("[LoRa] Sending ACK to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node));
  SendLoRaAndWait(controlPacket, CONTROL_SIZE);
  //Serial.printf("Sending ACK to %d.%d.%d\n", destination.region, destination.community, destination.node);
  Serial.write(hostACK, HOST_ACK_NACK_SIZE);
}

// Send a message to the specified destination
void SendMessage(nodeAddress destination, String message, String type) {
  dest = destination;
  
  // Prepare message data
  String messageData = type + ":" + message;
  int messageLength = messageData.length();
  
  // Account for header bytes (8 bytes: type + header + src + dest)
  int totalPacketLength = messageLength + 8;
  
  if (totalPacketLength > 255) {
    totalPacketLength = 255;
    messageLength = totalPacketLength - 8;
  }
  
  // Copy message to serial buffer (starting at offset 8 for header)
  messageData.getBytes(serialBuf + 8, messageLength + 1);
  
  // Display message on display
  DisplayTXMessage(message, dest);
  
  // Update packet header with addressing and send the message over the air
  serialBuf[0] = (uint8_t)MESSAGE;
  serialBuf[1] = 0;  // Header byte (not used)
  serialBuf[2] = local.region;
  serialBuf[3] = local.community;
  serialBuf[4] = local.node;
  serialBuf[5] = destination.region;
  serialBuf[6] = destination.community;
  serialBuf[7] = destination.node;
  
  addLog("[LoRa] Sending MESSAGE to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node) + " - Type: " + type + ", Length: " + String(totalPacketLength));
  addLog("[LoRa] Message content: " + messageData.substring(0, min(50, (int)messageData.length())) + "...");
  SendLoRaAndWait(serialBuf, (uint8_t)totalPacketLength);
  addLog("[LoRa] MESSAGE transmission completed");
}

// Send a multipart message to the specified destination
void SendMultipartMessage(nodeAddress destination, String message, String type) {
  // Prepare message data
  String messageData = type + ":" + message;
  int totalLength = messageData.length();
  
  // Calculate number of parts needed
  int totalParts = (totalLength + MULTIPART_CHUNK_SIZE - 1) / MULTIPART_CHUNK_SIZE;
  
  addLog("[LoRa] Sending MULTIPART MESSAGE to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node) + " - Type: " + type + ", Length: " + String(totalLength) + ", Parts: " + String(totalParts));
  
  for (int part = 0; part < totalParts; part++) {
    // Calculate chunk size
    int startPos = part * MULTIPART_CHUNK_SIZE;
    int chunkSize = min(MULTIPART_CHUNK_SIZE, totalLength - startPos);
    String chunk = messageData.substring(startPos, startPos + chunkSize);
    
    // Create multipart packet
    MultipartPacket mpPacket;
    mpPacket.packetType = (uint8_t)MULTIPART_PACKET;
    mpPacket.destRegion = destination.region;
    mpPacket.destCommunity = destination.community;
    mpPacket.destNode = destination.node;
    mpPacket.srcRegion = local.region;
    mpPacket.srcCommunity = local.community;
    mpPacket.srcNode = local.node;
    mpPacket.partNumber = part + 1;  // 1-based part numbering
    mpPacket.totalParts = totalParts;
    mpPacket.dataType = 1;  // Message
    mpPacket.reserved = 0;
    
    // Copy chunk data
    chunk.getBytes((unsigned char*)mpPacket.data, chunkSize + 1);
    
    // Convert to serial buffer format
    uint8_t packetSize = MULTIPART_HEADER_SIZE + chunkSize;
    uint8_t packet[256];
    packet[0] = mpPacket.packetType;
    packet[1] = mpPacket.destRegion;
    packet[2] = mpPacket.destCommunity;
    packet[3] = mpPacket.destNode;
    packet[4] = mpPacket.srcRegion;
    packet[5] = mpPacket.srcCommunity;
    packet[6] = mpPacket.srcNode;
    packet[7] = mpPacket.partNumber;
    packet[8] = mpPacket.totalParts;
    packet[9] = mpPacket.dataType;
    packet[10] = mpPacket.reserved;
    packet[11] = 0;  // Padding
    
    // Copy data
    for (int i = 0; i < chunkSize; i++) {
      packet[12 + i] = mpPacket.data[i];
    }
    
    // Display progress
    char tempBuf[64];
    sprintf(tempBuf, "MSG Part %i/%i", part + 1, totalParts);
    DisplayTXMessage(String(tempBuf), destination);
    
    // Send the packet
    addLog("[LoRa] Sending MULTIPART MESSAGE part " + String(part + 1) + "/" + String(totalParts) + " to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node));
    // v0.4.2 — wait for each part to reach the air. The fixed delay that used
    // to stand in for this was shorter than the airtime of the packet it was
    // pacing, so the next Radio.Send() overwrote a transmission still in
    // progress and the receiver saw a sequence with holes in it.
    SendLoRaAndWait(packet, packetSize);
  }
  
  addLog("[LoRa] MULTIPART MESSAGE completed - " + String(totalParts) + " parts sent");
  isLoRaIdle = true;
}

// Send a transaction to the specified destination
void SendTransaction(nodeAddress destination, String transaction, String type) {
  dest = destination;
  
  // Prepare transaction data
  String txData = type + ":" + transaction;
  int txLength = txData.length();
  
  if (txLength > 255) {
    txLength = 255;
  }
  
  // Copy transaction to serial buffer
  txData.getBytes(serialBuf, txLength + 1);
  
  // Display transaction on display
  DisplayTXMessage("TX: " + type, dest);
  
  // Update packet header and send the transaction over the air
  serialBuf[0] = (uint8_t)TRANSACTION;
  addLog("[LoRa] Sending TRANSACTION to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node) + " - Type: " + type + ", Length: " + String(txLength));
  SendLoRaAndWait(serialBuf, (uint8_t)txLength);
}

// Send a large transaction using multipart packets
void SendMultipartTransaction(nodeAddress destination, String transaction, String type) {
  dest = destination;
  
  // Prepare transaction data
  String txData = type + ":" + transaction;
  int totalLength = txData.length();
  
  // Calculate number of parts needed
  int totalParts = (totalLength + MULTIPART_CHUNK_SIZE - 1) / MULTIPART_CHUNK_SIZE;
  
  if (totalParts > MAX_MULTIPART_PARTS) {
    addLog("[ERROR] Transaction too large for multipart - " + String(totalLength) + " bytes, max " + String(MAX_MULTIPART_PARTS * MULTIPART_CHUNK_SIZE) + " bytes");
    return;
  }
  
  addLog("[LoRa] Sending MULTIPART TRANSACTION to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node) + " - Type: " + type + ", Length: " + String(totalLength) + ", Parts: " + String(totalParts));
  
  // Send each part
  for (int part = 0; part < totalParts; part++) {
    int startPos = part * MULTIPART_CHUNK_SIZE;
    int chunkSize = min(MULTIPART_CHUNK_SIZE, totalLength - startPos);
    String chunk = txData.substring(startPos, startPos + chunkSize);
    
    // Create multipart packet
    MultipartPacket mpPacket;
    mpPacket.packetType = (uint8_t)MULTIPART_PACKET;
    mpPacket.destRegion = destination.region;
    mpPacket.destCommunity = destination.community;
    mpPacket.destNode = destination.node;
    mpPacket.srcRegion = local.region;
    mpPacket.srcCommunity = local.community;
    mpPacket.srcNode = local.node;
    mpPacket.partNumber = part + 1;  // 1-based part numbering
    mpPacket.totalParts = totalParts;
    mpPacket.dataType = 0;  // Transaction
    mpPacket.reserved = 0;
    
    // Copy chunk data
    chunk.getBytes((unsigned char*)mpPacket.data, chunkSize + 1);
    
    // Convert to serial buffer format
    uint8_t packetSize = MULTIPART_HEADER_SIZE + chunkSize;
    uint8_t packet[256];
    packet[0] = mpPacket.packetType;
    packet[1] = mpPacket.destRegion;
    packet[2] = mpPacket.destCommunity;
    packet[3] = mpPacket.destNode;
    packet[4] = mpPacket.srcRegion;
    packet[5] = mpPacket.srcCommunity;
    packet[6] = mpPacket.srcNode;
    packet[7] = mpPacket.partNumber;
    packet[8] = mpPacket.totalParts;
    packet[9] = mpPacket.dataType;
    packet[10] = mpPacket.reserved;
    packet[11] = 0;  // Padding
    
    // Copy data
    for (int i = 0; i < chunkSize; i++) {
      packet[12 + i] = mpPacket.data[i];
    }
    
    // Display progress
    char tempBuf[64];
    sprintf(tempBuf, "TX Part %i/%i", part + 1, totalParts);
    DisplayTXMessage(String(tempBuf), dest);
    
    // Send the packet
    addLog("[LoRa] Sending MULTIPART part " + String(part + 1) + "/" + String(totalParts) + " to " + String(destination.region) + "." + String(destination.community) + "." + String(destination.node));
    // v0.4.2 — wait for each part to reach the air rather than guessing at a
    // delay; see SendLoRaAndWait.
    SendLoRaAndWait(packet, packetSize);
  }
  
  addLog("[LoRa] MULTIPART TRANSACTION completed - " + String(totalParts) + " parts sent");
  isLoRaIdle = true;
}

// Send a large broadcast using multipart packets
void SendMultipartBroadcast(String message, String type, String priority, uint8_t hops) {
  // Prepare broadcast data
  String broadcastData = type + ":" + priority + ":" + message;
  int totalLength = broadcastData.length();

  // Calculate number of parts needed
  int totalParts = (totalLength + MULTIPART_CHUNK_SIZE - 1) / MULTIPART_CHUNK_SIZE;

  if (totalParts > MAX_MULTIPART_PARTS) {
    addLog("[ERROR] Broadcast too large for multipart - " + String(totalLength) + " bytes, max " + String(MAX_MULTIPART_PARTS * MULTIPART_CHUNK_SIZE) + " bytes");
    return;
  }

  addLog("[LoRa] Sending MULTIPART BROADCAST - Type: " + type + ", Priority: " + priority + ", Length: " + String(totalLength) + ", Parts: " + String(totalParts) + ", Hops: " + String(hops));

  // Set destination as broadcast
  SetDestinationAsBroadcast();

  // Send each part
  for (int part = 0; part < totalParts; part++) {
    int startPos = part * MULTIPART_CHUNK_SIZE;
    int chunkSize = min(MULTIPART_CHUNK_SIZE, totalLength - startPos);
    String chunk = broadcastData.substring(startPos, startPos + chunkSize);

    // Create multipart packet
    MultipartPacket mpPacket;
    mpPacket.packetType = (uint8_t)MULTIPART_PACKET;
    mpPacket.destRegion = 255;  // Broadcast
    mpPacket.destCommunity = 255;
    mpPacket.destNode = 255;
    mpPacket.srcRegion = local.region;
    mpPacket.srcCommunity = local.community;
    mpPacket.srcNode = local.node;
    mpPacket.partNumber = part + 1;  // 1-based part numbering
    mpPacket.totalParts = totalParts;
    mpPacket.dataType = 2;  // Broadcast
    mpPacket.reserved = hops;  // v0.4.0 — carry the mesh hop count in the reserved byte
    
    // Copy chunk data
    chunk.getBytes((unsigned char*)mpPacket.data, chunkSize + 1);
    
    // Convert to serial buffer format
    uint8_t packetSize = MULTIPART_HEADER_SIZE + chunkSize;
    uint8_t packet[256];
    packet[0] = mpPacket.packetType;
    packet[1] = mpPacket.destRegion;
    packet[2] = mpPacket.destCommunity;
    packet[3] = mpPacket.destNode;
    packet[4] = mpPacket.srcRegion;
    packet[5] = mpPacket.srcCommunity;
    packet[6] = mpPacket.srcNode;
    packet[7] = mpPacket.partNumber;
    packet[8] = mpPacket.totalParts;
    packet[9] = mpPacket.dataType;
    packet[10] = mpPacket.reserved;
    packet[11] = 0;  // Padding
    
    // Copy data
    for (int i = 0; i < chunkSize; i++) {
      packet[12 + i] = mpPacket.data[i];
    }
    
    // Display progress
    char tempBuf[64];
    sprintf(tempBuf, "BC Part %i/%i", part + 1, totalParts);
    DisplayBroadcastMessage(String(tempBuf), local);
    
    // Send the packet
    addLog("[LoRa] Sending MULTIPART BROADCAST part " + String(part + 1) + "/" + String(totalParts));
    // v0.4.2 — wait for each part to reach the air rather than guessing at a
    // delay; see SendLoRaAndWait.
    SendLoRaAndWait(packet, packetSize);
  }
  
  addLog("[LoRa] MULTIPART BROADCAST completed - " + String(totalParts) + " parts sent");
  addLog("[LoRa] Setting isLoRaIdle = true to enable RX mode");
  isLoRaIdle = true;
}

// Multipart packet reassembly functions
int FindMultipartSession(uint8_t srcRegion, uint8_t srcCommunity, uint8_t srcNode, uint8_t dataType) {
  for (int i = 0; i < activeMultipartSessions; i++) {
    if (multipartBuffer[i].srcRegion == srcRegion && 
        multipartBuffer[i].srcCommunity == srcCommunity && 
        multipartBuffer[i].srcNode == srcNode &&
        multipartBuffer[i].dataType == dataType) {
      return i;
    }
  }
  return -1;  // Not found
}

int CreateMultipartSession(uint8_t srcRegion, uint8_t srcCommunity, uint8_t srcNode, uint8_t totalParts, uint8_t dataType, uint8_t hops) {
  // Note: `MAX_MULTIPART_SESSIONS` is how many senders may be mid-sequence at
  // once, which is a different quantity from how many parts one sequence has —
  // they merely share a value. Each session reserves a full reassembly buffer
  // (MAX_MULTIPART_PARTS × MULTIPART_CHUNK_SIZE = 4000 bytes), so the table
  // costs about 82 KB of static RAM.
  if (activeMultipartSessions >= MAX_MULTIPART_SESSIONS) {
    addLog("[ERROR] No space for new multipart session");
    return -1;
  }

  int index = activeMultipartSessions++;
  multipartBuffer[index].srcRegion = srcRegion;
  multipartBuffer[index].srcCommunity = srcCommunity;
  multipartBuffer[index].srcNode = srcNode;
  multipartBuffer[index].totalParts = totalParts;
  multipartBuffer[index].receivedParts = 0;
  multipartBuffer[index].dataType = dataType;
  multipartBuffer[index].hops = hops;  // v0.4.0 — from the multipart `reserved` byte
  multipartBuffer[index].startTime = millis();
  
  // Initialize parts received array
  for (int i = 0; i < MAX_MULTIPART_PARTS; i++) {
    multipartBuffer[index].partsReceived[i] = false;
  }
  
  addLog("[LoRa] Created multipart session for " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode) + " - " + String(totalParts) + " parts");
  return index;
}

bool ProcessMultipartPacket() {
  // v0.4.2 — Validate the header before it is used to index anything.
  //
  // Every field below arrives over the air from an unauthenticated sender. The
  // previous version created the session first and only then checked the part
  // number, and never checked `totalParts` at all — so a packet claiming 255
  // parts wrote `partsReceived[254]` into a 20-element array and
  // `partSizes[254]` into a 20-element array, corrupting whatever followed them
  // in memory. Anyone within radio range could send that.
  if (rxSize < MULTIPART_HEADER_SIZE) {
    addLog("[ERROR] Multipart packet too short: " + String(rxSize) + " bytes");
    return false;
  }

  // Extract packet information
  uint8_t srcRegion = rxPacket[4];
  uint8_t srcCommunity = rxPacket[5];
  uint8_t srcNode = rxPacket[6];
  uint8_t partNumber = rxPacket[7];
  uint8_t totalParts = rxPacket[8];
  uint8_t dataType = rxPacket[9];
  uint8_t hops = rxPacket[10];  // v0.4.0 — mesh hop count carried in the `reserved` byte

  if (totalParts < 1 || totalParts > MAX_MULTIPART_PARTS) {
    addLog("[ERROR] Invalid multipart total: " + String(totalParts) +
           " (max " + String(MAX_MULTIPART_PARTS) + ") - dropping");
    return false;
  }
  if (partNumber < 1 || partNumber > totalParts) {
    addLog("[ERROR] Invalid part number: " + String(partNumber) + " (max: " + String(totalParts) + ")");
    return false;
  }

  int dataSize = rxSize - MULTIPART_HEADER_SIZE;
  if (dataSize > MULTIPART_CHUNK_SIZE) {
    // A part longer than a chunk would overwrite the start of the next part's
    // slot, and its recorded size would then push the reassembled total past the
    // assembly buffer.
    addLog("[ERROR] Multipart chunk of " + String(dataSize) + " bytes exceeds " +
           String(MULTIPART_CHUNK_SIZE) + " - dropping");
    return false;
  }

  // Find or create session
  int sessionIndex = FindMultipartSession(srcRegion, srcCommunity, srcNode, dataType);
  if (sessionIndex != -1 && multipartBuffer[sessionIndex].totalParts != totalParts) {
    // A sender reusing the same (source, type) key with a different part count
    // means the previous sequence was abandoned. Start over rather than mixing
    // two payloads into one buffer.
    addLog("[LoRa] Multipart part count changed for " + String(srcRegion) + "." +
           String(srcCommunity) + "." + String(srcNode) + " - restarting session");
    RemoveMultipartSession(sessionIndex);
    sessionIndex = -1;
  }
  if (sessionIndex == -1) {
    sessionIndex = CreateMultipartSession(srcRegion, srcCommunity, srcNode, totalParts, dataType, hops);
    if (sessionIndex == -1) {
      addLog("[LoRa] Failed to create multipart session - buffer full");
      return false;
    }
    addLog("[LoRa] Created multipart session for " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode) + " - " + String(totalParts) + " parts, Type: " + String(dataType));
    addLog("[DEBUG] Session created for part " + String(partNumber) + " of " + String(totalParts));
  } else {
    addLog("[LoRa] Using existing multipart session for " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode) + " - " + String(multipartBuffer[sessionIndex].receivedParts) + "/" + String(multipartBuffer[sessionIndex].totalParts) + " parts, Type: " + String(dataType));
    addLog("[DEBUG] Session exists, receiving part " + String(partNumber) + " of " + String(totalParts));
  }

  // Check if we already have this part
  if (multipartBuffer[sessionIndex].partsReceived[partNumber - 1]) {
    addLog("[LoRa] Duplicate part " + String(partNumber) + " received, ignoring");
    return true;
  }
  
  // Log which parts we already have
  String receivedParts = "";
  for (int i = 0; i < multipartBuffer[sessionIndex].totalParts; i++) {
    if (multipartBuffer[sessionIndex].partsReceived[i]) {
      receivedParts += String(i + 1) + ",";
    }
  }
  addLog("[DEBUG] Already received parts: " + receivedParts + " now receiving part " + String(partNumber));
  
  // Extract data from packet (starts at offset 12). `dataSize` was validated
  // against MULTIPART_CHUNK_SIZE above, and `partNumber` against `totalParts`,
  // so this write stays inside the part's own slot.
  int startPos = (partNumber - 1) * MULTIPART_CHUNK_SIZE;

  // Copy data to reassembly buffer
  for (int i = 0; i < dataSize && (startPos + i) < (MAX_MULTIPART_PARTS * MULTIPART_CHUNK_SIZE); i++) {
    multipartBuffer[sessionIndex].assembledData[startPos + i] = rxPacket[12 + i];
  }
  
  // Store actual part size
  multipartBuffer[sessionIndex].partSizes[partNumber - 1] = dataSize;
  
  // Mark part as received
  multipartBuffer[sessionIndex].partsReceived[partNumber - 1] = true;
  multipartBuffer[sessionIndex].receivedParts++;
  
  addLog("[LoRa] Received multipart part " + String(partNumber) + "/" + String(totalParts) + " from " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode) + " (Data size: " + String(dataSize) + " bytes)");
  
  // Check if all parts received
  if (multipartBuffer[sessionIndex].receivedParts == totalParts) {
    addLog("[LoRa] All multipart parts received, reassembling...");
    return ReassembleMultipartPacket(sessionIndex);
  }
  
  return true;
}

bool ReassembleMultipartPacket(int sessionIndex) {
  MultipartReassembly* session = &multipartBuffer[sessionIndex];

  // v0.4.2 — `totalParts` is validated on arrival, but assert it here too: this
  // function indexes three fixed-size arrays with it, and it is the last place
  // that could stop a malformed session from reading past them.
  if (session->totalParts < 1 || session->totalParts > MAX_MULTIPART_PARTS) {
    addLog("[ERROR] Refusing to reassemble session with " + String(session->totalParts) + " parts");
    RemoveMultipartSession(sessionIndex);
    return false;
  }

  // Create final assembled data using a proper buffer.
  //
  // The size calculation that used to precede this loop called strlen() on
  // binary chunk data — reading past the end of a chunk that contained no NUL —
  // and then threw the answer away, because the copy below tracks the real
  // length itself. It is gone.
  static const int ASSEMBLED_CAPACITY = MAX_MULTIPART_PARTS * MULTIPART_CHUNK_SIZE;
  char assembledBuffer[ASSEMBLED_CAPACITY + 1];
  int assembledLength = 0;

  for (int part = 0; part < session->totalParts; part++) {
    if (session->partsReceived[part]) {
      int startPos = part * MULTIPART_CHUNK_SIZE;
      int partSize = session->partSizes[part]; // Use stored actual part size
      addLog("[DEBUG] Part " + String(part + 1) + " size: " + String(partSize) + " bytes");

      // Never write past the assembly buffer, whatever the recorded sizes say.
      if (partSize < 0) partSize = 0;
      if (assembledLength + partSize > ASSEMBLED_CAPACITY) {
        partSize = ASSEMBLED_CAPACITY - assembledLength;
        addLog("[ERROR] Reassembly buffer full at part " + String(part + 1) + " - truncating");
      }

      // Copy binary data properly to buffer
      for (int i = 0; i < partSize; i++) {
        assembledBuffer[assembledLength + i] = session->assembledData[startPos + i];
      }
      assembledLength += partSize;
    }
  }
  // Create String from buffer with proper length
  String assembledData = "";
  assembledData.reserve(assembledLength);
  for (int i = 0; i < assembledLength; i++) {
    assembledData += (char)assembledBuffer[i];
  }
  
  addLog("[LoRa] Reassembled multipart data - " + String(assembledLength) + " bytes, Type: " + String(session->dataType) + " from " + String(session->srcRegion) + "." + String(session->srcCommunity) + "." + String(session->srcNode));
  addLog("[DEBUG] Assembled data (first 50 chars): " + assembledData.substring(0, min(50, (int)assembledData.length())));
  addLog("[DEBUG] Assembled data (last 50 chars): " + assembledData.substring(max(0, (int)assembledData.length() - 50)));
  
  // Process based on data type
  switch (session->dataType) {
    case 0:  // Transaction
      ProcessReassembledTransaction(assembledData, session->srcRegion, session->srcCommunity, session->srcNode, session->hops);
      break;
    case 1:  // Message
      ProcessReassembledMessage(assembledData, session->srcRegion, session->srcCommunity, session->srcNode, session->hops);
      break;
    case 2:  // Broadcast
      ProcessReassembledBroadcast(assembledData, session->srcRegion, session->srcCommunity, session->srcNode, session->hops);
      break;
    default:
      addLog("[ERROR] Unknown multipart data type: " + String(session->dataType));
      break;
  }
  
  // Remove session
  RemoveMultipartSession(sessionIndex);
  return true;
}

void ProcessReassembledTransaction(String transactionData, uint8_t srcRegion, uint8_t srcCommunity, uint8_t srcNode, uint8_t hops) {
  addLog("[LoRa] Processing reassembled TRANSACTION from " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode));
  
  // Set sender address
  senderAddress.region = srcRegion;
  senderAddress.community = srcCommunity;
  senderAddress.node = srcNode;
  
  // Display on screen
  DisplayRXMessage("TX: " + transactionData.substring(0, min(50, (int)transactionData.length())) + "...", senderAddress);
  
  // Extract transaction type and data
  int colonPos = transactionData.indexOf(':');
  String txType = "signed";
  String txData = transactionData;
  if (colonPos > 0) {
    txType = transactionData.substring(0, colonPos);
    txData = transactionData.substring(colonPos + 1);
  }
  
  // v0.4.2 — This legacy line of free-form text ("TRANSACTION:<data>") goes down the
  // same wire the host reads binary packets from, and the data is arbitrary: a
  // byte pair inside it can look like a packet header, after which the framer
  // consumes the real packet behind it. The web UI and GET /api/logs carry the
  // same information, so it is off unless HOST_SERIAL_DEBUG is set.
#if HOST_SERIAL_DEBUG
  String fullMessage = "TRANSACTION:" + transactionData;
  Serial.write(fullMessage.c_str(), fullMessage.length());
  Serial.write('\n');
#endif
  
  // Auto-forward to configured gateway if available
  String internetResponse = "";
  bool gateway_forwarded = false;
  
  if (gatewayForwardingEnabled() && gateway_type != "none" && gateway_ip.length() > 0) {
    String gatewayUrl = "http://" + gateway_ip + ":" + gateway_port;
    if (gateway_type != "core" && gateway_endpoint.length() > 0) {
      gatewayUrl += gateway_endpoint;
    }
    
    addLog("[GATEWAY] Forwarding transaction to " + gateway_type + " gateway at " + gatewayUrl);
    addLog("[GATEWAY] Transaction data length: " + String(txData.length()) + " bytes");
    addLog("[GATEWAY] Transaction type: " + txType);
    addLog("[GATEWAY] Transaction data (first 50 chars): " + txData.substring(0, min(50, (int)txData.length())));
    addLog("[GATEWAY] Transaction data (last 50 chars): " + txData.substring(max(0, (int)txData.length() - 50)));
    
    if (gateway_type == "core") {
      // Use JSON-RPC for Dogecoin Core
      addLog("[GATEWAY] Using JSON-RPC method for Dogecoin Core");
      internetResponse = sendJsonRpcToGateway(txData, gatewayUrl, gateway_username, gateway_password);
      addLog("[GATEWAY] Dogecoin Core response: " + internetResponse);
      gateway_forwarded = true;
    } else {
      // Use regular HTTP POST for other gateways
      addLog("[GATEWAY] Using HTTP POST method for " + gateway_type);
      internetResponse = sendTransactionToCustomGateway(txData, gatewayUrl);
      addLog("[GATEWAY] " + gateway_type + " response: " + internetResponse);
      gateway_forwarded = true;
    }
  } else if (gatewayForwardingEnabled() && internet_connected) {
    // Fallback to default internet gateway
    addLog("[GATEWAY] No configured gateway, using default internet gateway (BlockCypher)");
    addLog("[GATEWAY] Transaction data length: " + String(txData.length()) + " bytes");
    internetResponse = sendTransactionToInternet(txData);
    addLog("[GATEWAY] BlockCypher response: " + internetResponse);
    gateway_forwarded = true;
  } else {
    addLog("[GATEWAY] No gateway configured and no internet connection available");
  }
  
  if (gateway_forwarded) {
    addLog("[LoRa] Gateway forwarding successful, sending confirmation back to " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode));
    addLog("[LoRa] Gateway response: " + internetResponse);
    
    // Send confirmation back to original sender via LoRa
    String confirmMessage = "DOGECOIN_RESPONSE:" + internetResponse;
    nodeAddress originalSender = {srcRegion, srcCommunity, srcNode};
    addLog("[LoRa] Sending Dogecoin node response to original sender: " + confirmMessage);
    
    // Small delay to ensure sender is ready to receive
    delay(2000);
    
    // Check if message is too long for regular message
    if (confirmMessage.length() > 240) {
      addLog("[LoRa] Confirmation message too long, using multipart");
      SendMultipartMessage(originalSender, confirmMessage, "confirmation");
    } else {
      addLog("[LoRa] Using regular message for confirmation");
      SendMessage(originalSender, confirmMessage, "confirmation");
    }
    
    // Send a second confirmation after a delay to ensure delivery
    delay(3000);
    addLog("[LoRa] Sending second confirmation to ensure delivery");
    if (confirmMessage.length() > 240) {
      addLog("[LoRa] Second confirmation also using multipart");
      SendMultipartMessage(originalSender, confirmMessage, "confirmation");
    } else {
      SendMessage(originalSender, confirmMessage, "confirmation");
    }
  } else {
    addLog("[LoRa] No gateway forwarding performed - transaction stored locally only");
    
    // Rebroadcast transaction to other LoRa devices for mesh networking
    if (ENABLE_MESH_REBROADCAST && hops >= MAX_REBROADCAST_HOPS) {
      // v0.4.0 — hop limit reached; drop instead of rebroadcasting to bound mesh storms
      addLog("[LoRa] Mesh hop limit (" + String(MAX_REBROADCAST_HOPS) + ") reached at hop " + String(hops) + " - not rebroadcasting");
    } else if (ENABLE_MESH_REBROADCAST) {
      addLog("[LoRa] Rebroadcasting transaction to other LoRa devices for mesh networking (hop " + String(hops + 1) + ")");
      String rebroadcastData = "transaction:" + txType + ":" + txData;
      // Always rebroadcast via multipart so the incremented hop counter (carried
      // in the multipart `reserved` byte) survives the next relay; single-packet
      // broadcasts have no header room for it.
      SendMultipartBroadcast(rebroadcastData, "transaction", "normal", hops + 1);
    } else {
      addLog("[LoRa] Mesh rebroadcasting disabled - transaction stored locally only");
    }
  }
  
  addLog("[LoRa] Reassembled transaction processed and forwarded to host" + String(gateway_forwarded ? " and gateway" : ""));
}

void ProcessReassembledMessage(String messageData, uint8_t srcRegion, uint8_t srcCommunity, uint8_t srcNode, uint8_t hops) {
  addLog("[LoRa] Processing reassembled MESSAGE from " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode));
  
  // Set sender address
  senderAddress.region = srcRegion;
  senderAddress.community = srcCommunity;
  senderAddress.node = srcNode;
  
  // Check if this is a Dogecoin node response
  if (messageData.startsWith("DOGECOIN_RESPONSE:")) {
    addLog("[LoRa] Received Dogecoin node response from " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode));
    addLog("[LoRa] Dogecoin response: " + messageData);
    addLog("[LoRa] Full Dogecoin node message received and displayed on screen");
    
    // Handle confirmation for queued requests
    HandleConfirmationReceived(messageData);
  }
  
  // Display on screen
  DisplayRXMessage(messageData, senderAddress);
  
  // v0.4.2 — This legacy line of free-form text ("MESSAGE:<data>") goes down the
  // same wire the host reads binary packets from, and the data is arbitrary: a
  // byte pair inside it can look like a packet header, after which the framer
  // consumes the real packet behind it. The web UI and GET /api/logs carry the
  // same information, so it is off unless HOST_SERIAL_DEBUG is set.
#if HOST_SERIAL_DEBUG
  String fullMessage = "MESSAGE:" + messageData;
  Serial.write(fullMessage.c_str(), fullMessage.length());
  Serial.write('\n');
#endif
  
  addLog("[LoRa] Reassembled message forwarded to host");
}

// FNV-1a 32-bit hash for broadcast deduplication
static uint32_t fnv1a(const String& s) {
  uint32_t h = 2166136261u;
  for (int i = 0; i < (int)s.length(); i++) {
    h ^= (uint8_t)s.charAt(i);
    h *= 16777619u;
  }
  return h;
}

// Returns true if an identical broadcast from this source was recently processed.
// Records the entry on first sight.
// v0.4.2 — Keyed on the payload alone, not on (source, payload).
//
// A relayed broadcast is retransmitted with the *relay's* address as its source
// (SendMultipartBroadcast writes `local` into the multipart header), so the same
// original payload arrives from a different "source" at every hop. Keying on the
// source therefore meant a node never recognised a payload it had already seen
// and relayed, and flood suppression did nothing: with N nodes in range the same
// transaction reached the gateway once per path through the mesh, and was
// submitted to the Dogecoin network once per arrival. The hop limit bounded the
// storm but did not stop the duplication.
//
// Suppressing an identical payload from any source within the TTL is exactly what
// flood suppression should do — two nodes sending byte-identical data inside two
// minutes are relaying the same thing. The source is kept in the table for the
// log line only.
bool isRecentlySeenBroadcast(uint8_t srcRegion, uint8_t srcCommunity, uint8_t srcNode, const String& data) {
  uint32_t hash = fnv1a(data);
  unsigned long now = millis();

  // Compact out expired entries
  int writeIdx = 0;
  for (int i = 0; i < seenBroadcastCount; i++) {
    if (now - seenBroadcasts[i].seenAt < SEEN_BROADCAST_TTL_MS) {
      seenBroadcasts[writeIdx++] = seenBroadcasts[i];
    }
  }
  seenBroadcastCount = writeIdx;

  // Check for duplicate
  for (int i = 0; i < seenBroadcastCount; i++) {
    if (seenBroadcasts[i].dataHash == hash) {
      return true;
    }
  }

  // New entry — record it, evict oldest if full
  if (seenBroadcastCount < MAX_SEEN_BROADCASTS) {
    seenBroadcasts[seenBroadcastCount++] = {srcRegion, srcCommunity, srcNode, hash, now};
  } else {
    int oldest = 0;
    for (int i = 1; i < MAX_SEEN_BROADCASTS; i++) {
      if (seenBroadcasts[i].seenAt < seenBroadcasts[oldest].seenAt) oldest = i;
    }
    seenBroadcasts[oldest] = {srcRegion, srcCommunity, srcNode, hash, now};
  }
  return false;
}

void ProcessReassembledBroadcast(String broadcastData, uint8_t srcRegion, uint8_t srcCommunity, uint8_t srcNode, uint8_t hops) {
  // Suppress duplicate deliveries from mesh rebroadcasting
  if (isRecentlySeenBroadcast(srcRegion, srcCommunity, srcNode, broadcastData)) {
    addLog("[LoRa] Duplicate broadcast from " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode) + " suppressed");
    return;
  }
  addLog("[LoRa] Processing reassembled BROADCAST from " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode));
  
  // Set sender address
  senderAddress.region = srcRegion;
  senderAddress.community = srcCommunity;
  senderAddress.node = srcNode;
  
  // Display on screen
  DisplayBroadcastMessage(broadcastData, senderAddress);
  
  // v0.4.2 — This legacy line of free-form text ("BROADCAST:<data>") goes down the
  // same wire the host reads binary packets from, and the data is arbitrary: a
  // byte pair inside it can look like a packet header, after which the framer
  // consumes the real packet behind it. The web UI and GET /api/logs carry the
  // same information, so it is off unless HOST_SERIAL_DEBUG is set.
#if HOST_SERIAL_DEBUG
  String fullMessage = "BROADCAST:" + broadcastData;
  Serial.write(fullMessage.c_str(), fullMessage.length());
  Serial.write('\n');
#endif
  
  // Auto-forward to configured gateway if available
  String internetResponse = "";
  bool gateway_forwarded = false;
  
  // v0.4.2 — Parse the broadcast envelope generically.
  //
  // A broadcast payload is "<type>:<priority>:<data>". The old code only
  // recognised the literal prefix "transaction:", so anything else — an
  // announcement, say — fell through with its type and priority left at the
  // hardcoded defaults. It was then relayed as "transaction:normal:" plus the
  // *entire* original payload, and forwarded to the Dogecoin network as if the
  // whole thing were a raw transaction.
  String bcType = "transaction";
  String bcPriority = "normal";
  String txData = broadcastData;
  {
    int c1 = broadcastData.indexOf(':');
    int c2 = (c1 > 0) ? broadcastData.indexOf(':', c1 + 1) : -1;
    if (c1 > 0 && c2 > c1) {
      bcType     = broadcastData.substring(0, c1);
      bcPriority = broadcastData.substring(c1 + 1, c2);
      txData     = broadcastData.substring(c2 + 1);
    }
  }

  // Only a transaction belongs on the Dogecoin network. Posting an announcement
  // to a node as a raw transaction can only ever be rejected, and on a metered
  // or rate-limited gateway it is not free.
  bool isTransaction = (bcType == "transaction");
  if (!isTransaction) {
    addLog("[GATEWAY] Broadcast type '" + bcType + "' is not a transaction - not forwarding");
  }

  if (isTransaction && gatewayForwardingEnabled() && gateway_type != "none" && gateway_ip.length() > 0) {
    String gatewayUrl = "http://" + gateway_ip + ":" + gateway_port;
    if (gateway_type != "core" && gateway_endpoint.length() > 0) {
      gatewayUrl += gateway_endpoint;
    }
    
    addLog("[GATEWAY] Forwarding broadcast to " + gateway_type + " gateway at " + gatewayUrl);
    
    addLog("[GATEWAY] Broadcast transaction data length: " + String(txData.length()) + " bytes");
    addLog("[GATEWAY] Transaction data (first 50 chars): " + txData.substring(0, min(50, (int)txData.length())));
    addLog("[GATEWAY] Transaction data (last 50 chars): " + txData.substring(max(0, (int)txData.length() - 50)));
    
    if (gateway_type == "core") {
      // Use JSON-RPC for Dogecoin Core
      addLog("[GATEWAY] Using JSON-RPC method for Dogecoin Core");
      internetResponse = sendJsonRpcToGateway(txData, gatewayUrl, gateway_username, gateway_password);
      addLog("[GATEWAY] Dogecoin Core response: " + internetResponse);
      gateway_forwarded = true;
    } else {
      // Use regular HTTP POST for other gateways
      addLog("[GATEWAY] Using HTTP POST method for " + gateway_type);
      internetResponse = sendTransactionToCustomGateway(txData, gatewayUrl);
      addLog("[GATEWAY] " + gateway_type + " response: " + internetResponse);
      gateway_forwarded = true;
    }
  } else if (isTransaction && gatewayForwardingEnabled() && internet_connected) {
    // Fallback to default internet gateway
    addLog("[GATEWAY] No configured gateway, using default internet gateway (BlockCypher)");
    addLog("[GATEWAY] Broadcast transaction data length: " + String(txData.length()) + " bytes");
    internetResponse = sendTransactionToInternet(txData);
    addLog("[GATEWAY] BlockCypher response: " + internetResponse);
    gateway_forwarded = true;
  } else {
    addLog("[GATEWAY] No gateway configured and no internet connection available");
  }
  
  if (gateway_forwarded) {
    addLog("[LoRa] Gateway forwarding successful, sending confirmation back to " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode));
    addLog("[LoRa] Gateway response: " + internetResponse);
    
    // Send confirmation back to original sender via LoRa
    String confirmMessage = "DOGECOIN_RESPONSE:" + internetResponse;
    nodeAddress originalSender = {srcRegion, srcCommunity, srcNode};
    addLog("[LoRa] Sending Dogecoin node response to original sender: " + confirmMessage);
    addLog("[LoRa] Original sender address: " + String(srcRegion) + "." + String(srcCommunity) + "." + String(srcNode));
    addLog("[LoRa] Dogecoin response length: " + String(confirmMessage.length()) + " bytes");
    
    // Small delay to ensure sender is ready to receive
    delay(2000);
    
    // Check if message is too long for regular message
    if (confirmMessage.length() > 240) {
      addLog("[LoRa] Confirmation message too long, using multipart");
      SendMultipartMessage(originalSender, confirmMessage, "confirmation");
    } else {
      addLog("[LoRa] Using regular message for confirmation");
      SendMessage(originalSender, confirmMessage, "confirmation");
    }
    
    // Send a second confirmation after a delay to ensure delivery
    delay(3000);
    addLog("[LoRa] Sending second confirmation to ensure delivery");
    if (confirmMessage.length() > 240) {
      addLog("[LoRa] Second confirmation also using multipart");
      SendMultipartMessage(originalSender, confirmMessage, "confirmation");
    } else {
      SendMessage(originalSender, confirmMessage, "confirmation");
    }
  } else {
    addLog("[LoRa] No gateway forwarding performed - broadcast stored locally only");
    
    // Rebroadcast to other LoRa devices for mesh networking
    if (ENABLE_MESH_REBROADCAST && hops >= MAX_REBROADCAST_HOPS) {
      // v0.4.0 — hop limit reached; drop instead of rebroadcasting to bound mesh storms
      addLog("[LoRa] Mesh hop limit (" + String(MAX_REBROADCAST_HOPS) + ") reached at hop " + String(hops) + " - not rebroadcasting");
    } else if (ENABLE_MESH_REBROADCAST) {
      addLog("[LoRa] Rebroadcasting to other LoRa devices for mesh networking (hop " + String(hops + 1) + ")");
      // Relay it as what it is. bcType/bcPriority came from the envelope above,
      // so a non-transaction broadcast keeps its own type instead of being
      // relabelled as a transaction carrying its own header as payload.
      String rebroadcastType = bcType;
      String rebroadcastPriority = bcPriority;
      String rebroadcastData = rebroadcastType + ":" + rebroadcastPriority + ":" + txData;
      // Always rebroadcast via multipart so the incremented hop counter (carried
      // in the multipart `reserved` byte) survives the next relay.
      SendMultipartBroadcast(rebroadcastData, rebroadcastType, rebroadcastPriority, hops + 1);
    } else {
      addLog("[LoRa] Mesh rebroadcasting disabled - broadcast stored locally only");
    }
  }
  
  addLog("[LoRa] Reassembled broadcast forwarded to host and gateway");
}

void RemoveMultipartSession(int sessionIndex) {
  if (sessionIndex < 0 || sessionIndex >= activeMultipartSessions) return;
  
  // Shift remaining sessions down
  for (int i = sessionIndex; i < activeMultipartSessions - 1; i++) {
    multipartBuffer[i] = multipartBuffer[i + 1];
  }
  
  activeMultipartSessions--;
  addLog("[LoRa] Removed multipart session " + String(sessionIndex));
}

// API handler for multipart status
void handleApiMultipartStatus() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"active_sessions\":" + String(activeMultipartSessions) + ",";
  response += "\"max_sessions\":" + String(MAX_MULTIPART_SESSIONS) + ",";
  response += "\"chunk_size\":" + String(MULTIPART_CHUNK_SIZE) + ",";
  response += "\"max_parts\":" + String(MAX_MULTIPART_PARTS) + ",";
  response += "\"timeout_ms\":" + String(MULTIPART_TIMEOUT_MS) + ",";
  response += "\"sessions\":[";
  
  for (int i = 0; i < activeMultipartSessions; i++) {
    if (i > 0) response += ",";
    response += "{";
    response += "\"src\":\"" + String(multipartBuffer[i].srcRegion) + "." + String(multipartBuffer[i].srcCommunity) + "." + String(multipartBuffer[i].srcNode) + "\",";
    response += "\"total_parts\":" + String(multipartBuffer[i].totalParts) + ",";
    response += "\"received_parts\":" + String(multipartBuffer[i].receivedParts) + ",";
    response += "\"data_type\":" + String(multipartBuffer[i].dataType) + ",";
    response += "\"age_ms\":" + String(millis() - multipartBuffer[i].startTime);
    response += "}";
  }
  
  response += "]";
  response += "}";
  
  server.send(200, "application/json", response);
}

void CleanupExpiredMultipartSessions() {
  unsigned long currentTime = millis();
  
  for (int i = activeMultipartSessions - 1; i >= 0; i--) {
    if (currentTime - multipartBuffer[i].startTime > MULTIPART_TIMEOUT_MS) {
      addLog("[LoRa] Multipart session expired for " + String(multipartBuffer[i].srcRegion) + "." + String(multipartBuffer[i].srcCommunity) + "." + String(multipartBuffer[i].srcNode) + " - Received " + String(multipartBuffer[i].receivedParts) + "/" + String(multipartBuffer[i].totalParts) + " parts");
      RemoveMultipartSession(i);
    }
  }
}

// Request queuing and confirmation system functions
String GenerateRequestId() {
  requestIdCounter++;
  return "req_" + String(millis()) + "_" + String(requestIdCounter);
}

bool QueueRequest(RequestType type, String message, String typeStr, String priority, nodeAddress destination = {0, 0, 0}, bool requiresConfirmation = true) {
  if (pendingRequestCount >= MAX_PENDING_REQUESTS) {
    addLog("[ERROR] Request queue is full - cannot queue new request");
    return false;
  }
  
  PendingRequest& req = pendingRequests[pendingRequestCount];
  req.type = type;
  req.message = message;
  req.typeStr = typeStr;
  req.priority = priority;
  req.destination = destination;
  req.timestamp = millis();
  req.requiresConfirmation = requiresConfirmation;
  req.requestId = GenerateRequestId();
  
  // Determine if multipart is needed
  String testData = typeStr + ":" + priority + ":" + message;
  req.isMultipart = (testData.length() > 255);
  
  pendingRequestCount++;
  
  addLog("[QUEUE] Request queued - ID: " + req.requestId + ", Type: " + String(type) + ", Queue size: " + String(pendingRequestCount));
  return true;
}

// Forward declarations for process functions defined below
void ProcessBroadcastRequest(PendingRequest& req);
void ProcessTransactionRequest(PendingRequest& req);
void ProcessMessageRequest(PendingRequest& req);
void ProcessPingRequest(PendingRequest& req);

// v0.4.2 — Rewritten to dequeue before dispatching, and to iterate rather than
// recurse.
//
// The old shape had two defects that compounded:
//
//   1. The request was removed from the queue *after* its handler returned. A
//      handler for a request with requiresConfirmation == false called straight
//      back into this function, which then found the same request still sitting
//      at pendingRequests[0] with the state still IDLE — and dispatched it
//      again. Every level of that recursion transmitted over LoRa, and it ended
//      only when the stack ran out and the board reset.
//   2. Being re-entrant at all was unsafe: a confirmation arriving mid-dispatch
//      (LoRa RX is serviced from inside SendLoRaAndWait) calls
//      HandleConfirmationReceived, which also calls this function.
//
// Now: take the request off the queue first, dispatch it, and either stop
// because it is waiting for a confirmation or continue round the loop. A guard
// makes re-entry a no-op instead of a second concurrent walk of the queue.
void ProcessNextQueuedRequest() {
  static bool processing = false;
  if (processing) {
    addLog("[QUEUE] Re-entrant call ignored - already processing the queue");
    return;
  }
  processing = true;

  while (true) {
    if (pendingRequestCount == 0) {
      currentRequestState = REQUEST_IDLE;
      addLog("[QUEUE] No more requests in queue - returning to idle state");
      break;
    }

    if (currentRequestState != REQUEST_IDLE) {
      addLog("[QUEUE] Not idle - leaving the rest of the queue for later");
      break;
    }

    // Copy the request out and remove it from the queue *before* dispatching, so
    // no path can ever see it as still pending.
    PendingRequest req = pendingRequests[0];
    for (int i = 0; i < pendingRequestCount - 1; i++) {
      pendingRequests[i] = pendingRequests[i + 1];
    }
    pendingRequestCount--;

    currentRequestId = req.requestId;
    addLog("[QUEUE] Processing request - ID: " + req.requestId + ", Type: " + String(req.type));

    switch (req.type) {
      case REQUEST_BROADCAST:
        ProcessBroadcastRequest(req);
        break;
      case REQUEST_TRANSACTION:
        ProcessTransactionRequest(req);
        break;
      case REQUEST_MESSAGE:
        ProcessMessageRequest(req);
        break;
      case REQUEST_PING:
        ProcessPingRequest(req);
        break;
    }

    // A request awaiting confirmation owns the queue until it is confirmed or
    // times out; anything else falls through to the next one.
    if (currentRequestState == REQUEST_WAITING_FOR_CONFIRMATION) {
      break;
    }
  }

  processing = false;
}

void ProcessBroadcastRequest(PendingRequest& req) {
  if (req.isMultipart) {
    // Locally-originated broadcast starts at hop 0.
    SendMultipartBroadcast(req.message, req.typeStr, req.priority, 0);
    addLog("[QUEUE] Sent multipart broadcast - ID: " + req.requestId);
  } else {
    SendBroadcast(req.message, req.typeStr, req.priority);
    addLog("[QUEUE] Sent regular broadcast - ID: " + req.requestId);
  }
  
  if (req.requiresConfirmation) {
    currentRequestState = REQUEST_WAITING_FOR_CONFIRMATION;
    confirmationStartTime = millis();
    addLog("[QUEUE] Waiting for confirmation - ID: " + req.requestId);
  }
  // Nothing else to do: ProcessNextQueuedRequest drives the queue and moves
  // on by itself when this request needs no confirmation. Calling back into
  // it from here re-dispatched this very request, forever.
}

void ProcessTransactionRequest(PendingRequest& req) {
  if (req.isMultipart) {
    SendMultipartTransaction(req.destination, req.message, req.typeStr);
    addLog("[QUEUE] Sent multipart transaction - ID: " + req.requestId);
  } else {
    SendTransaction(req.destination, req.message, req.typeStr);
    addLog("[QUEUE] Sent regular transaction - ID: " + req.requestId);
  }
  
  if (req.requiresConfirmation) {
    currentRequestState = REQUEST_WAITING_FOR_CONFIRMATION;
    confirmationStartTime = millis();
    addLog("[QUEUE] Waiting for confirmation - ID: " + req.requestId);
  }
  // Nothing else to do: ProcessNextQueuedRequest drives the queue and moves
  // on by itself when this request needs no confirmation. Calling back into
  // it from here re-dispatched this very request, forever.
}

void ProcessMessageRequest(PendingRequest& req) {
  if (req.isMultipart) {
    SendMultipartMessage(req.destination, req.message, req.typeStr);
    addLog("[QUEUE] Sent multipart message - ID: " + req.requestId);
  } else {
    SendMessage(req.destination, req.message, req.typeStr);
    addLog("[QUEUE] Sent regular message - ID: " + req.requestId);
  }
  
  if (req.requiresConfirmation) {
    currentRequestState = REQUEST_WAITING_FOR_CONFIRMATION;
    confirmationStartTime = millis();
    addLog("[QUEUE] Waiting for confirmation - ID: " + req.requestId);
  }
  // Nothing else to do: ProcessNextQueuedRequest drives the queue and moves
  // on by itself when this request needs no confirmation. Calling back into
  // it from here re-dispatched this very request, forever.
}

void ProcessPingRequest(PendingRequest& req) {
  SendPing(req.destination);
  addLog("[QUEUE] Sent ping - ID: " + req.requestId);
  
  if (req.requiresConfirmation) {
    currentRequestState = REQUEST_WAITING_FOR_CONFIRMATION;
    confirmationStartTime = millis();
    addLog("[QUEUE] Waiting for ACK - ID: " + req.requestId);
  }
  // Nothing else to do: ProcessNextQueuedRequest drives the queue and moves
  // on by itself when this request needs no confirmation. Calling back into
  // it from here re-dispatched this very request, forever.
}

void CheckRequestTimeouts() {
  unsigned long currentTime = millis();
  
  // Check confirmation timeout
  if (currentRequestState == REQUEST_WAITING_FOR_CONFIRMATION) {
    if (currentTime - confirmationStartTime > CONFIRMATION_TIMEOUT_MS) {
      addLog("[QUEUE] Confirmation timeout for request - ID: " + currentRequestId);
      currentRequestState = REQUEST_IDLE;
      ProcessNextQueuedRequest();
    }
  }
  
  // Check for expired requests in queue
  for (int i = pendingRequestCount - 1; i >= 0; i--) {
    if (currentTime - pendingRequests[i].timestamp > REQUEST_TIMEOUT_MS) {
      addLog("[QUEUE] Request expired and removed - ID: " + pendingRequests[i].requestId);
      // Remove expired request
      for (int j = i; j < pendingRequestCount - 1; j++) {
        pendingRequests[j] = pendingRequests[j + 1];
      }
      pendingRequestCount--;
    }
  }
}

// v0.4.2 — Ignore a confirmation identical to one just accepted.
//
// Confirmations carry no request id, so any DOGECOIN_RESPONSE completes whatever
// request happens to be waiting. That collides with the gateway deliberately
// sending each confirmation twice for reliability (see
// ProcessReassembledTransaction): the first copy completed the request it was
// meant for, the queue moved on and started the next one, and the second copy
// arrived moments later and completed *that* one too — a request whose reply had
// not come back yet, and never would be waited for.
//
// Remembering the last confirmation for the length of a confirmation window is
// enough to tell a retransmission from a genuine second answer, and keeps the
// redundant send doing what it was meant to do.
void HandleConfirmationReceived(String confirmationData) {
  static uint32_t lastConfirmationHash = 0;
  static unsigned long lastConfirmationAt = 0;
  static bool haveLastConfirmation = false;

  if (currentRequestState != REQUEST_WAITING_FOR_CONFIRMATION) {
    return;
  }

  uint32_t hash = fnv1a(confirmationData);
  unsigned long now = millis();
  if (haveLastConfirmation && hash == lastConfirmationHash &&
      (now - lastConfirmationAt) < CONFIRMATION_TIMEOUT_MS) {
    addLog("[QUEUE] Duplicate confirmation ignored - it does not belong to " + currentRequestId);
    return;
  }
  lastConfirmationHash = hash;
  lastConfirmationAt = now;
  haveLastConfirmation = true;

  addLog("[QUEUE] Confirmation received for request - ID: " + currentRequestId + ", Data: " + confirmationData.substring(0, min(50, (int)confirmationData.length())));
  currentRequestState = REQUEST_IDLE;
  ProcessNextQueuedRequest();
}

// Send a broadcast message
void SendBroadcast(String message, String type, String priority) {
  // Prepare broadcast data
  String broadcastData = type + ":" + priority + ":" + message;
  int broadcastLength = broadcastData.length();
  
  if (broadcastLength > 255) {
    broadcastLength = 255;
  }
  
  // Copy broadcast to serial buffer
  broadcastData.getBytes(serialBuf, broadcastLength + 1);
  
  // Display broadcast on display
  DisplayBroadcastMessage(message, local);
  
  // Update packet header and send the broadcast over the air
  serialBuf[0] = (uint8_t)BROADCAST;
  addLog("[LoRa] Sending BROADCAST - Type: " + type + ", Priority: " + priority + ", Length: " + String(broadcastLength));
  SendLoRaAndWait(serialBuf, (uint8_t)broadcastLength);
}

// Basically we will just send out the the serial buffer
// Change up the first byte to indicate messageType=MESSAGE
void SendMessageFromBuffer(int messageLength) {
  // For now we will cap the size of the message we can send
  // Will have to investigate breaking up large messages in future but Radio.Send only accepts a uint8_t as the buffer size
  if (messageLength > 255) {
    messageLength = 255;
  }

  // Display message on display
  String messageString = ExtractStringMessageFromBuffer(serialBuf, messageLength);
  DisplayTXMessage(messageString, dest);

  // Update packet header and send the message over the air
  serialBuf[0] = (uint8_t)MESSAGE;
  SendLoRaAndWait(serialBuf, (uint8_t)messageLength);
}

// Read the serial header and extract the command type and payload size from it.
// Currently the header consists of 2 bytes: the command type and the payload size.
bool ReadSerialHeader(serialCommand &commandType, uint8_t &payloadSize) {
  size_t numRead = Serial.readBytes(serialHeader, SERIAL_HEADER_SIZE);
  if (numRead != SERIAL_HEADER_SIZE) {
    return false;
  }
  // First byte will be command
  // Next byte will be payload size
  commandType = (serialCommand)serialHeader[0];
  payloadSize = serialHeader[1];
  return true;
}

// v0.4.2 — Read exactly `n` bytes from the host, waiting up to `timeoutMs` in
// total for them to arrive.
//
// Serial.readBytes() has its own stream timeout, but it is a global setting the
// rest of this sketch also depends on; taking the wait explicitly keeps the
// framing rules for a host frame in one readable place and makes a truncated
// frame a definite failure rather than a partial read nobody checks.
bool ReadHostBytes(uint8_t *dst, size_t n, uint32_t timeoutMs) {
  if (n == 0) {
    return true;
  }
  size_t got = 0;
  uint32_t start = millis();
  while (got < n && (millis() - start) < timeoutMs) {
    int avail = Serial.available();
    if (avail <= 0) {
      delay(1);
      continue;
    }
    size_t want = n - got;
    if ((size_t)avail < want) {
      want = (size_t)avail;
    }
    got += Serial.readBytes(dst + got, want);
  }
  return got == n;
}

// v0.4.2 — Read a host payload that carries no length of its own.
//
// The desktop protocol has no size field for CMD_DOGE_TX / CMD_REQUEST_BALANCE
// single packets or for a relayed CMD_MESSAGE, so the only boundary available is
// a gap in the byte stream. This waits briefly for the payload to start, then
// consumes bytes until the host has been quiet for HOST_FRAME_QUIET_MS.
//
// What it deliberately does *not* do is drain everything currently buffered,
// which is what the previous implementation did after a fixed delay(500). That
// glued any command queued behind this one onto its payload and then discarded
// it — so two commands sent close together lost the second, and a multipart
// sequence arrived as one oversized, unparseable frame.
//
// Returns the number of bytes read (0 if the payload never started).
uint8_t ReadHostPayloadUntilQuiet(uint8_t *dst, uint8_t maxLen) {
  uint8_t len = 0;
  uint32_t start = millis();
  // Wait for the first byte.
  while (Serial.available() <= 0 && (millis() - start) < HOST_PAYLOAD_START_TIMEOUT_MS) {
    delay(1);
  }
  uint32_t lastByte = millis();
  while (len < maxLen) {
    if (Serial.available() > 0) {
      dst[len++] = (uint8_t)Serial.read();
      lastByte = millis();
      continue;
    }
    if ((millis() - lastByte) >= HOST_FRAME_QUIET_MS) {
      break;
    }
    delay(1);
  }
  return len;
}

// Read the host payload into the serial buffer.
bool ReadSerialPayload(uint8_t payloadSize) {
  uint8_t bytesRead = Serial.readBytes(serialBuf, payloadSize);
  if (bytesRead != payloadSize) {
    return false;
  }
  //Serial.printf("Read %d bytes\n", bytesRead);
  return true;
}

// OPTIMIZED FOR DESKTOP v0.3.3 – SAFE
// Handles desktop-app command IDs that are not present in the firmware's serialCommand enum.
// The desktop uses an 8-byte packet header: [cmd, flags=0x00, src_r, src_c, src_n, dst_r, dst_c, dst_n, ...payload...]
// HostSerialRead() consumes the first 2 bytes as [cmd, payloadSize=0x00=flags] and reads 0 payload bytes.
// After delay(500), the remaining 6 header bytes plus any actual payload sit in the Serial buffer.
// This function drains those bytes and sends responses in the same 8-byte desktop format.
//
// Handled desktop commands (all additive — existing firmware command IDs unchanged):
//   0x10  CMD_DOGE_TX             — forward payload bytes over LoRa, reply ACK
//   0x11  CMD_REQUEST_BALANCE     — ACK (balance fulfillment happens via LoRa network)
//   0x20  CMD_GET_FIRMWARE_VERSION — reply with version string in payload
//   0x21  CMD_SET_LORA_PARAMS     — ACK (RF reconfiguration reserved for future revision)
// Execute a desktop-protocol command.
//
// v0.4.1 — This used to read its own bytes straight off `Serial`, which meant
// only USB could ever reach it: BLE writes landed in bleRxBuffer and were never
// consumed, so commands sent over Bluetooth did nothing. The bytes are now
// passed in, so USB and BLE share one implementation of every command rather
// than growing a second copy that drifts.
//
//   flags        header byte 1: low nibble 0x1 = multipart, high nibble = hops
//   hdrRest      the 6 header bytes after [cmd, flags]:
//                [src_r, src_c, src_n, dst_r, dst_c, dst_n]
//   extraPayload payload bytes following the 8-byte header. For a multipart
//                frame this starts with [total, index, session_hi, session_lo].
//   extraLen     number of valid bytes in extraPayload
//
// Replies already go to both transports (Serial.write + bleSend), so a command
// arriving over either link is answered on both.
void HandleDesktopCommand(uint8_t cmdByte, uint8_t flags, const uint8_t* hdrRest, const uint8_t* extraPayload, uint8_t extraLen) {
  switch (cmdByte) {
    case 0x10: { // CMD_DOGE_TX — relay a signed transaction over LoRa.
      // v0.4.0 (WP1): transmit the FULL desktop-format packet — command byte plus
      // 8-byte header — so a receiving gateway can hand it to its serial host
      // (radiodoge-cli daemon) verbatim and broadcast it to the Dogecoin network.
      // Earlier firmware sent only the raw payload with no header, so a gateway
      // could not recognise or forward it. hdrRest holds [src_r,src_c,src_n,dst_r,dst_c,dst_n].
      if (extraLen > 0) {
        uint8_t ota[BUFFER_SIZE];
        // v0.4.1 — preserve the flags byte. It used to be hardcoded to 0x00,
        // which flattened FLAG_MULTIPART and made a multipart frame
        // indistinguishable from a truncated single packet on the air.
        ota[0] = 0x10; ota[1] = flags;
        for (int i = 0; i < 6; i++) ota[2 + i] = hdrRest[i];
        int payLen = extraLen;
        if (8 + payLen > BUFFER_SIZE) payLen = BUFFER_SIZE - 8;
        memcpy(ota + 8, extraPayload, payLen);
        int otaLen = 8 + payLen;
        if ((flags & 0x0F) == DESKTOP_FLAG_MULTIPART &&
            extraLen >= DESKTOP_MULTIPART_EXTRA) {
          nodeAddress mpDest = { hdrRest[3], hdrRest[4], hdrRest[5] };
          char partLabel[32];
          snprintf(partLabel, sizeof(partLabel), "TX part %u/%u",
                   (unsigned)(extraPayload[1] + 1), (unsigned)extraPayload[0]);
          DisplayTXMessage(String(partLabel), mpDest);
        }
        // v0.4.2 — block until the packet is actually on the air. This used to
        // set isLoRaIdle = true immediately, which made the next loop iteration
        // switch the radio to receive a few milliseconds into a transmission
        // that needs hundreds — so the transaction never left the board.
        SendLoRaAndWait(ota, (uint8_t)(otaLen > 255 ? 255 : otaLen));
        // v0.3.6 — trigger "TX OK!" OLED page after successful send
        showTxOk = true;
        txOkTimestamp = millis();
      }
      // The host treats this reply as its acknowledgement, and — because it is
      // written only after the transmission completed — as permission to send
      // the next frame of a multipart sequence.
      uint8_t reply[8] = {0x10, 0x00, local.region, local.community, local.node, 0xFF, 0xFF, 0xFF};
      Serial.write(reply, 8);
      bleSend(reply, 8);
      break;
    }
    case 0x11: { // CMD_REQUEST_BALANCE — relay over LoRa so a gateway can answer.
      // v0.4.0 (WP1): forward the full desktop packet over the air; the receiving
      // gateway hands it to its daemon, which looks up the balance and relays a
      // "BAL:<koinus>" MESSAGE back to the requester over LoRa.
      if (extraLen > 0) {
        uint8_t ota[BUFFER_SIZE];
        ota[0] = 0x11; ota[1] = flags;  // v0.4.1 — preserve multipart/hop flags
        for (int i = 0; i < 6; i++) ota[2 + i] = hdrRest[i];
        int payLen = extraLen;
        if (8 + payLen > BUFFER_SIZE) payLen = BUFFER_SIZE - 8;
        memcpy(ota + 8, extraPayload, payLen);
        int otaLen = 8 + payLen;
        // v0.4.2 — wait for the transmission to finish; see the 0x10 case.
        SendLoRaAndWait(ota, (uint8_t)(otaLen > 255 ? 255 : otaLen));
      }
      uint8_t reply[8] = {0x11, 0x00, local.region, local.community, local.node, 0xFF, 0xFF, 0xFF};
      Serial.write(reply, 8);
      bleSend(reply, 8);
      break;
    }
    case 0x20: { // CMD_GET_FIRMWARE_VERSION — reply with version string as payload
      char vStr[32];
      snprintf(vStr, sizeof(vStr), "RadioDoge NV%dFW%02d", HELTEC_BOARD_VERSION, FIRMWARE_VERSION);
      uint8_t vLen = (uint8_t)strlen(vStr);
      uint8_t reply[8 + 32];
      reply[0] = 0x20; reply[1] = 0x00;
      reply[2] = local.region; reply[3] = local.community; reply[4] = local.node;
      reply[5] = 0xFF; reply[6] = 0xFF; reply[7] = 0xFF;
      memcpy(reply + 8, vStr, vLen);
      Serial.write(reply, 8 + vLen);
      break;
    }
    case 0x21: { // CMD_SET_LORA_PARAMS — payload: [sf, bw_idx, cr, freq_khz(u32 BE), power]
      // v0.4.1 — This used to ACK without touching the radio, so the app could
      // "save" settings that never took effect. The radio is now really retuned.
      //
      // The frequency is a big-endian u32 in kHz occupying bytes [3..7], with TX
      // power at [7]. It was a 2-byte field until v0.4.1, which could not
      // represent 915000 kHz (20 bits) — no firmware ever read it, so the field
      // was widened into the two formerly-reserved bytes rather than kept.
      if (extraLen >= 8) {
        uint8_t  sf    = extraPayload[0];
        uint8_t  bw    = extraPayload[1];
        uint8_t  cr    = extraPayload[2];
        uint32_t f_khz = ((uint32_t)extraPayload[3] << 24) |
                         ((uint32_t)extraPayload[4] << 16) |
                         ((uint32_t)extraPayload[5] << 8)  |
                          (uint32_t)extraPayload[6];
        int8_t   power = (int8_t)extraPayload[7];

        // The desktop sends coding rate as the denominator (5..8); the radio
        // wants an index (1..4).
        if (cr >= 5 && cr <= 8) cr -= 4;

        uint32_t new_khz = f_khz;
        uint32_t freq_hz = new_khz * 1000UL;

        if (loRaParamsAreValid(freq_hz, sf, bw, cr, power)) {
          lora_freq_hz          = freq_hz;
          lora_spreading_factor = sf;
          lora_bandwidth        = bw;
          lora_coding_rate      = cr;
          lora_tx_power         = power;
          applyLoRaRadioConfig();
          saveLoRaRadioParamsQuiet();
          addLog("[LoRa] Retuned: SF" + String(sf) + " BW" + String(bw) +
                 " CR4/" + String(cr + 4) + " " + String(new_khz) + "kHz " +
                 String(power) + "dBm");
        } else {
          // Out of range — leave the radio exactly as it was. Replying with a
          // NACK lets the host surface a real failure instead of showing
          // "Verified" for settings that were silently discarded.
          addLog("[LoRa] Rejected out-of-range radio params from host");
          Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
          bleSend(hostNACK, HOST_ACK_NACK_SIZE);
          break;
        }
      }
      uint8_t reply[8] = {0x21, 0x00, local.region, local.community, local.node, 0xFF, 0xFF, 0xFF};
      Serial.write(reply, 8);
      bleSend(reply, 8);
      break;
    }

    // v0.3.6 — CMD_GET_SETTINGS: board reports live state to app on connect.
    // Response payload (4 bytes): [node_region, node_community, node_node, gateway_mode]
    // App parses this and syncs UI — board is source of truth.
    case 0x22: {
      // v0.3.8 — reply[13]: header(8) + [region, community, node, gateway_mode, wifi_enabled](5)
      uint8_t reply[13];
      reply[0] = 0x22; reply[1] = 0x00;
      reply[2] = local.region; reply[3] = local.community; reply[4] = local.node;
      reply[5] = 0xFF; reply[6] = 0xFF; reply[7] = 0xFF;
      reply[8] = local.region; reply[9] = local.community; reply[10] = local.node;
      reply[11] = gateway_mode ? 1u : 0u;
      reply[12] = wifi_enabled ? 1u : 0u;
      Serial.write(reply, 13);
      bleSend(reply, 13);
      break;
    }

    // v0.3.6 — CMD_SET_GATEWAY: persist gateway_mode to NVS; reply with current state.
    // Payload byte 0: 1 = enable, 0 = disable.
    case 0x23: {
      if (extraLen > 0) {
        gateway_mode = (extraPayload[0] != 0);
        saveGatewayModeQuiet(gateway_mode);
      }
      uint8_t payload[1] = {gateway_mode ? 1u : 0u};
      uint8_t reply[9];
      reply[0] = 0x23; reply[1] = 0x00;
      reply[2] = local.region; reply[3] = local.community; reply[4] = local.node;
      reply[5] = 0xFF; reply[6] = 0xFF; reply[7] = 0xFF;
      reply[8] = gateway_mode ? 1 : 0;
      Serial.write(reply, 9);
      bleSend(reply, 9);
      break;
    }

    // v0.3.7 — CMD_WIFI_TOGGLE (0x24): toggle WiFi radio on/off. Payload byte 0: 1=on, 0=off.
    // Persists to NVS; OLED icon row updates on next status cycle.
    case 0x24: {
      if (extraLen > 0) {
        wifi_enabled = (extraPayload[0] != 0);
        saveWifiEnabledQuiet(wifi_enabled);
        if (wifi_enabled) {
          setupDualWiFi();
        } else {
          WiFi.disconnect(true);
          WiFi.mode(WIFI_OFF);
          addLog("[WiFi] WiFi radio disabled by host command");
        }
      }
      uint8_t reply[9];
      reply[0] = 0x24; reply[1] = 0x00;
      reply[2] = local.region; reply[3] = local.community; reply[4] = local.node;
      reply[5] = 0xFF; reply[6] = 0xFF; reply[7] = 0xFF;
      reply[8] = wifi_enabled ? 1 : 0;
      Serial.write(reply, 9);
      bleSend(reply, 9);
      break;
    }

    // v0.3.8 — CMD_GET_BATTERY (0x26): Read ADC and report battery voltage in mV.
    // Heltec V3: ADC_CRTL=GPIO37 (low-active enable), BAT_ADC=GPIO1, divider ratio=2.
    case 0x26: {
      #if HELTEC_BOARD_VERSION == 3
        pinMode(37, OUTPUT);
        digitalWrite(37, LOW);   // enable battery measurement
        delay(5);
        int raw = analogRead(1);
        digitalWrite(37, HIGH);  // disable after reading (saves power)
        uint16_t mv = (uint16_t)((float)raw / 4095.0f * 3300.0f * 2.0f);
      #else
        uint16_t mv = 0; // V2 board: no dedicated battery ADC
      #endif
      uint8_t reply[10];
      reply[0] = 0x26; reply[1] = 0x00;
      reply[2] = local.region; reply[3] = local.community; reply[4] = local.node;
      reply[5] = 0xFF; reply[6] = 0xFF; reply[7] = 0xFF;
      reply[8] = (uint8_t)(mv >> 8);   // high byte
      reply[9] = (uint8_t)(mv & 0xFF); // low byte
      Serial.write(reply, 10);
      bleSend(reply, 10);
      break;
    }

    // v0.4.0 — CMD_BLE_TOGGLE (0x28): enable/disable BLE advertising. Payload byte 0: 1=on, 0=off.
    // Persists to NVS so the choice survives reboot; reply payload byte reports current state.
    case 0x28: {
      if (extraLen > 0) {
        ble_enabled = (extraPayload[0] != 0);
        saveBleEnabledQuiet(ble_enabled);
        bleSetAdvertising(ble_enabled);
        addLog(String("[BLE] Advertising ") + (ble_enabled ? "enabled" : "disabled") + " by host command");
      }
      uint8_t reply[9];
      reply[0] = 0x28; reply[1] = 0x00;
      reply[2] = local.region; reply[3] = local.community; reply[4] = local.node;
      reply[5] = 0xFF; reply[6] = 0xFF; reply[7] = 0xFF;
      reply[8] = ble_enabled ? 1 : 0;
      Serial.write(reply, 9);
      bleSend(reply, 9);
      break;
    }

    // v0.3.8 — CMD_GET_MAC (0x27): Report the WiFi station MAC address (6 bytes).
    case 0x27: {
      uint8_t mac[6] = {0};
      esp_read_mac(mac, ESP_MAC_WIFI_STA);
      uint8_t reply[14];
      reply[0] = 0x27; reply[1] = 0x00;
      reply[2] = local.region; reply[3] = local.community; reply[4] = local.node;
      reply[5] = 0xFF; reply[6] = 0xFF; reply[7] = 0xFF;
      memcpy(reply + 8, mac, 6);
      Serial.write(reply, 14);
      bleSend(reply, 14);
      break;
    }

    default:
      Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
      bleSend(hostNACK, HOST_ACK_NACK_SIZE);
      break;
  }
}

// v0.4.0 (WP1) — Relay a desktop CMD_MESSAGE (0x03) from the host out over LoRa.
// The daemon replies to a relayed transaction or balance request with a desktop
// CMD_MESSAGE ("TX_ACK:<txid>" / "BAL:<koinus>") addressed to the originating
// node; a gateway must put that message on the air so it reaches the requester.
// The full 8-byte desktop packet is transmitted verbatim (CMD_MESSAGE == the
// firmware's messageType MESSAGE == 0x03), so the destination node forwards it to
// its own host app, which parses the TX_ACK:/BAL: prefix.
void RelayDesktopMessageOverLoRa() {
  // ReadSerialHeader already consumed [0x03, 0x00]. The rest of the frame is
  // [src_r, src_c, src_n, dst_r, dst_c, dst_n, ...text...]; the six address
  // bytes have a known length, the text does not, so it ends at a gap in the
  // byte stream (v0.4.2 — this used to drain everything buffered, which glued a
  // second acknowledgement queued behind the first onto its text).
  uint8_t rest[BUFFER_SIZE];
  if (!ReadHostBytes(rest, 6, HOST_FRAME_TIMEOUT_MS)) {
    Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
    return;
  }
  uint8_t textLen = ReadHostPayloadUntilQuiet(rest + 6, (uint8_t)(BUFFER_SIZE - 8 - 6));

  uint8_t ota[BUFFER_SIZE];
  ota[0] = 0x03;  // CMD_MESSAGE == messageType MESSAGE
  ota[1] = 0x00;
  for (int i = 0; i < 6; i++) ota[2 + i] = rest[i];  // src(3) + dst(3)
  for (int i = 0; i < textLen; i++) ota[8 + i] = rest[6 + i];
  int otaLen = 8 + textLen;
  nodeAddress relayDest = { ota[5], ota[6], ota[7] };
  DisplayTXMessage("Relay", relayDest);
  // v0.4.2 — wait for the transmission rather than declaring the radio idle and
  // letting the main loop abort it. This is the path that carries TX_ACK back to
  // the sender, so aborting it left a broadcast transaction unacknowledged.
  SendLoRaAndWait(ota, (uint8_t)(otaLen > 255 ? 255 : otaLen));
  addLog("[GATEWAY] Relayed host MESSAGE over LoRa to " + String(ota[5]) + "." + String(ota[6]) + "." + String(ota[7]) + " (" + String(otaLen) + " bytes)");
  Serial.write(hostACK, HOST_ACK_NACK_SIZE);
}

// ─── v0.4.1 — BLE command execution ──────────────────────────────────────────
//
// Inbound BLE writes used to accumulate in bleRxBuffer and were never read, so
// every command sent over Bluetooth was silently ignored — the app could
// connect and receive notifications but not control the board. This drains the
// buffer and runs the same HandleDesktopCommand the USB path uses.
//
// Framing: a packet is dispatched once either its length is known and has
// arrived (desktopCommandLength, or a multipart frame's declared chunk length)
// or — for the variable-length commands, which carry no length anywhere — the
// link has been quiet for BLE_FRAME_QUIET_MS.
//
// v0.4.2 — raised from 60 ms. A BLE link starts at the mandatory 23-byte ATT
// MTU and the host cannot negotiate or even query a larger one, so it writes a
// packet as a run of 20-byte chunks. Each is a separate acknowledged GATT
// operation subject to the connection interval and the phone's scheduler, and a
// gap of well over 60 ms between two chunks of the *same* packet is ordinary.
// At 60 ms the board dispatched the first half of a transaction as if it were a
// whole one.
//
// This only delays the variable-length commands (0x10 / 0x11 as single packets).
// Anything with a known length — every fixed-size command, and every multipart
// frame — is dispatched the moment its last byte arrives, no waiting at all.
#define BLE_FRAME_QUIET_MS 400

#if ENABLE_BLE
void ProcessBleCommands() {
  if (!blePendingData) return;
  if (bleRxLen < SINGLE_PACKET_HEADER_SIZE) return;  // header still arriving

  uint8_t cmdByte = bleRxBuffer[0];

  // Anything we cannot execute is discarded rather than left to accumulate —
  // otherwise one stray byte wedges the 256-byte buffer permanently.
  if (!isDesktopCommandByte(cmdByte)) {
    addLog("[BLE] Dropping unrecognised command 0x" + String(cmdByte, HEX));
    bleRxLen = 0;
    blePendingData = false;
    return;
  }

  uint16_t need;
  if ((bleRxBuffer[1] & 0x0F) == DESKTOP_FLAG_MULTIPART) {
    // v0.4.2 — a multipart frame declares its own chunk length, so it is framed
    // exactly rather than by a quiet gap. Without this a sequence written as
    // fast as GATT allows arrives as one run of bytes with no boundaries in it.
    const int mpHdr = SINGLE_PACKET_HEADER_SIZE + DESKTOP_MULTIPART_EXTRA;
    if (bleRxLen < mpHdr) return;  // header still arriving
    uint8_t chunkLen = bleRxBuffer[mpHdr - 1];
    if (chunkLen > DESKTOP_MULTIPART_CHUNK_MAX) {
      addLog("[BLE] Multipart chunk length " + String(chunkLen) + " out of range — dropping");
      bleRxLen = 0;
      blePendingData = false;
      return;
    }
    need = (uint16_t)(mpHdr + chunkLen);
    if (bleRxLen < (int)need) return;  // chunk still arriving
  } else {
    need = desktopCommandLength(cmdByte);
    if (need > 0) {
      if (bleRxLen < (int)need) return;  // fixed-length packet still arriving
    } else {
      // Variable length: no in-band size, so a quiet gap marks the boundary.
      if (millis() - lastBleRxMillis < BLE_FRAME_QUIET_MS) return;
      need = (uint16_t)bleRxLen;
    }
  }

  // Capture the header fields before the buffer is shifted below.
  uint8_t flags = bleRxBuffer[1];
  uint8_t hdrRest[6];
  memcpy(hdrRest, bleRxBuffer + 2, 6);

  uint8_t payloadLen = (uint8_t)(need - SINGLE_PACKET_HEADER_SIZE);
  uint8_t extraPayload[BUFFER_SIZE];
  memset(extraPayload, 0, sizeof(extraPayload));
  if (payloadLen > 0) {
    memcpy(extraPayload, bleRxBuffer + SINGLE_PACKET_HEADER_SIZE, payloadLen);
  }

  // Consume this packet, preserving anything that arrived behind it.
  if (bleRxLen > (int)need) {
    memmove(bleRxBuffer, bleRxBuffer + need, bleRxLen - need);
    bleRxLen -= (int)need;
  } else {
    bleRxLen = 0;
    blePendingData = false;
  }

  addLog("[BLE] Executing desktop cmd 0x" + String(cmdByte, HEX) +
         " (" + String(payloadLen) + " payload bytes)");
  HandleDesktopCommand(cmdByte, flags, hdrRest, extraPayload, payloadLen);
}
#else
void ProcessBleCommands() {}
#endif

// Read serial data from the host and perform the specified command/control function.
// This function expects the host to send a header that is 8 bytes in length and then a payload that can range from 0-255 bytes.
// The header contains the command type and payload size information (see ReadSerialHeader for more info on the header)
void HostSerialRead() {
  serialCommand commandVal;
  uint8_t payloadSize;
  bool headerSuccess = ReadSerialHeader(commandVal, payloadSize);
  if (!headerSuccess) {
    return;
  }
  hostCommandReply[0] = commandVal;
  uint8_t cmdByte = (uint8_t)commandVal;

  // ── v0.4.1 — Desktop commands are dispatched BEFORE ReadSerialPayload ───────
  //
  // ReadSerialHeader treats byte 1 as a payload length, but the desktop protocol
  // puts its FLAGS byte there. That only ever worked because flags are normally
  // 0x00, which reads as "no payload". A multipart frame sets FLAG_MULTIPART
  // (0x01), so ReadSerialPayload used to consume one byte — the first byte of
  // the source address — and every byte after it was misaligned. That is why a
  // host could not send multipart to a board, which in turn capped host->board
  // payloads at a single 192-byte packet.
  //
  // Reading the desktop dispatch first means byte 1 is never interpreted as a
  // length for these commands, so any flags value frames correctly.
  if (isDesktopCommandByte(cmdByte)) {
    uint8_t flags = payloadSize;  // byte 1 is FLAGS for desktop commands

    // The 6 remaining header bytes: [src_r, src_c, src_n, dst_r, dst_c, dst_n].
    uint8_t hdrRest[6] = {0};
    if (!ReadHostBytes(hdrRest, 6, HOST_FRAME_TIMEOUT_MS)) {
      addLog("[HOST] Truncated desktop header for cmd 0x" + String(cmdByte, HEX));
      Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
      bleSend(hostNACK, HOST_ACK_NACK_SIZE);
      return;
    }

    // Everything after the 8-byte header, read to an exact length wherever the
    // protocol defines one. The old code slept 500 ms and then swallowed every
    // buffered byte, which merged whatever the host sent next into this frame.
    uint8_t extraPayload[BUFFER_SIZE];
    memset(extraPayload, 0, sizeof(extraPayload));
    uint8_t extraLen = 0;

    if ((flags & 0x0F) == DESKTOP_FLAG_MULTIPART) {
      // Multipart: five header bytes ending in an explicit chunk length, then
      // exactly that many payload bytes. This is what makes a transaction
      // spanning several frames arrive intact instead of glued together.
      if (!ReadHostBytes(extraPayload, DESKTOP_MULTIPART_EXTRA, HOST_FRAME_TIMEOUT_MS)) {
        addLog("[HOST] Truncated multipart header for cmd 0x" + String(cmdByte, HEX));
        Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
        bleSend(hostNACK, HOST_ACK_NACK_SIZE);
        return;
      }
      uint8_t chunkLen = extraPayload[DESKTOP_MULTIPART_EXTRA - 1];
      if (chunkLen > DESKTOP_MULTIPART_CHUNK_MAX) {
        addLog("[HOST] Multipart chunk length " + String(chunkLen) + " out of range");
        Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
        bleSend(hostNACK, HOST_ACK_NACK_SIZE);
        return;
      }
      if (!ReadHostBytes(extraPayload + DESKTOP_MULTIPART_EXTRA, chunkLen, HOST_FRAME_TIMEOUT_MS)) {
        addLog("[HOST] Truncated multipart chunk (" + String(chunkLen) + " bytes expected)");
        Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
        bleSend(hostNACK, HOST_ACK_NACK_SIZE);
        return;
      }
      extraLen = (uint8_t)(DESKTOP_MULTIPART_EXTRA + chunkLen);
    } else {
      uint8_t fixedTotal = desktopCommandLength(cmdByte);
      if (fixedTotal > 0) {
        // A command with a known total size: read precisely its payload and
        // leave anything behind it for the next pass of the loop. Two commands
        // sent back to back are now both executed; previously the second was
        // consumed as payload of the first and lost.
        uint8_t want = (uint8_t)(fixedTotal - SINGLE_PACKET_HEADER_SIZE);
        if (want > 0 && !ReadHostBytes(extraPayload, want, HOST_FRAME_TIMEOUT_MS)) {
          addLog("[HOST] Truncated payload for cmd 0x" + String(cmdByte, HEX));
          Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
          bleSend(hostNACK, HOST_ACK_NACK_SIZE);
          return;
        }
        extraLen = want;
      } else {
        // No length anywhere in the protocol (single-packet 0x10 / 0x11), so
        // the frame ends at a gap in the byte stream.
        extraLen = ReadHostPayloadUntilQuiet(
            extraPayload, (uint8_t)(BUFFER_SIZE - SINGLE_PACKET_HEADER_SIZE));
      }
    }

    HandleDesktopCommand(cmdByte, flags, hdrRest, extraPayload, extraLen);
    return;
  }

  // v0.4.0 (WP1) — Gateway relay of a desktop MESSAGE (0x03) from the host over LoRa.
  // payloadSize == 0 (the desktop flags byte) distinguishes this from the legacy
  // PING_REQUEST(3) enum value, which carries a non-zero payload size.
  //
  // v0.4.2 — decided before ReadSerialPayload and without the blanket delay:
  // RelayDesktopMessageOverLoRa now frames the rest of the packet itself, and a
  // desktop MESSAGE has no legacy payload for ReadSerialPayload to consume.
  if (cmdByte == 0x03 && payloadSize == 0) {
    if (gateway_mode) {
      RelayDesktopMessageOverLoRa();
      return;
    }
    // v0.4.2 — Not a gateway, so there is nothing to relay. Consume the frame
    // and say so. Falling through would reach the legacy PING_REQUEST case
    // (enum value 3 collides with CMD_MESSAGE) and transmit a ping to whatever
    // address happened to be left in the serial buffer.
    uint8_t discard[BUFFER_SIZE];
    ReadHostPayloadUntilQuiet(discard, (uint8_t)(BUFFER_SIZE - 1));
    addLog("[HOST] Ignoring MESSAGE relay request — gateway mode is off");
    Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
    return;
  }

  // Now we will read in the host's payload
  bool payloadSuccess = ReadSerialPayload(payloadSize);
  if (!payloadSuccess) {
    Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
    return;
  }
  delay(500);

  switch (commandVal) {
    case NONE: // 0x00 = desktop CMD_GET_NODE_ADDR
      // OPTIMIZED FOR DESKTOP v0.3.3 – SAFE
      // Drain 6 remaining desktop header bytes; reply with local address in 8-byte format.
      // Desktop serial.rs checks: cmd==0x00 && src!=broadcast → updates displayed node address.
      {
        uint8_t hdrRest[6] = {0};
        Serial.readBytes(hdrRest, 6);
        uint8_t reply[8] = {0x00, 0x00, local.region, local.community, local.node, 0xFF, 0xFF, 0xFF};
        Serial.write(reply, 8);
      }
      break;
    case ADDRESS_GET: // 0x01 = desktop CMD_SET_NODE_ADDRS when payloadSize==0
      // OPTIMIZED FOR DESKTOP v0.3.3 – SAFE
      // Desktop packet: [0x01, 0x00, src_r, src_c, src_n, 0xFF, 0xFF, 0xFF, new_r, new_c, new_n]
      // After delay(500), 9 remaining bytes are in the buffer.
      // If payloadSize>0, fall back to legacy firmware ADDRESS_GET behavior.
      if (payloadSize == 0 && Serial.available() >= 9) {
        uint8_t hdrRest[9] = {0};
        Serial.readBytes(hdrRest, 9);
        // hdrRest[0..2]=src, hdrRest[3..5]=dst, hdrRest[6..8]=new node address
        local.region    = hdrRest[6];
        local.community = hdrRest[7];
        local.node      = hdrRest[8];
        InitControlMessages();
        DisplayLocalAddress(local);
        saveLoRaConfigurationQuiet(local.region, local.community, local.node);
        uint8_t reply[8] = {0x01, 0x00, local.region, local.community, local.node, 0xFF, 0xFF, 0xFF};
        Serial.write(reply, 8);
      } else {
        GetLocalAddress();
        DisplayLocalAddress(local);
      }
      break;
    case ADDRESS_SET: // 0x02 = desktop CMD_PING when payloadSize==0 (flags byte)
      // OPTIMIZED FOR DESKTOP v0.3.3 – SAFE
      // Desktop CMD_PING: [0x02, 0x00, src_r, src_c, src_n, dst_r, dst_c, dst_n]
      // payloadSize==0 means it came from desktop (flags byte); drain 6 header bytes and ACK.
      // payloadSize>0 means legacy firmware ADDRESS_SET; apply as before.
      if (payloadSize == 0) {
        uint8_t hdrRest[6] = {0};
        Serial.readBytes(hdrRest, 6);
        uint8_t reply[8] = {0x02, 0x00, local.region, local.community, local.node, 0xFF, 0xFF, 0xFF};
        Serial.write(reply, 8);
      } else {
        SetLocalAddressFromSerialBuffer(0);
        InitControlMessages();
        DisplayLocalAddress(local);
        saveLoRaConfigurationQuiet(local.region, local.community, local.node);
        Serial.write(hostACK, HOST_ACK_NACK_SIZE);
      }
      break;
    case PING_REQUEST:
      SetDestinationFromSerialBuffer(0);
      SendPing(dest);
      break;
    case MESSAGE_REQUEST:
      SetDestinationFromSerialBuffer(4);
      SendMessageFromBuffer(payloadSize);
      break;
    case HARDWARE_INFO:
      DisplayHardwareInfo(HELTEC_BOARD_VERSION);
      //Serial.printf("RD HT V%d FW01\n", HELTEC_BOARD_VERSION);
      SendHardwareInfoToHost();
      break;
    case DISPLAY_CONTROL:
      ProcessDisplayControl(payloadSize);
      Serial.write(hostACK, HOST_ACK_NACK_SIZE);
      break;
    case HOST_FORMED_PACKET:
      //ParseHostFormedPacket(payloadSize);
      SetDestinationFromSerialBuffer(5);
      DisplayTXMessage("Custom Packet!", dest);
      // Send out the host formed packet
      SendLoRaAndWait(serialBuf, payloadSize);
      // Acknowledge the host that we sent the packet
      Serial.write(hostACK, HOST_ACK_NACK_SIZE);
      break;
    case MULTIPART_PACKET:
      SetDestinationFromSerialBuffer(5);
      char tempBuf[64];
      sprintf(tempBuf, "Part %i of %i", serialBuf[10], serialBuf[11]);
      DisplayTXMessage(String(tempBuf), dest);
      SendLoRaAndWait(serialBuf, payloadSize);
      // Send ACK to let them know we sent out that part
      Serial.write(hostACK, HOST_ACK_NACK_SIZE);
      break;
    default:
      // Indicate that command was not understood (Send NACK)
      Serial.write(hostNACK, HOST_ACK_NACK_SIZE);
      break;
  }
}

void ProcessCoinDisplay(int payloadSize)
{
  if (payloadSize < 5)
  {
    return;
  }
  float coinAmount = 0;
  memcpy(&coinAmount, serialBuf + 1, sizeof(coinAmount));
  if (serialBuf[0] == RECEIVING_DISPLAY)
  {
    DrawReceivingCoinsImage(coinAmount);
  }
  else if (serialBuf[0] == SENDING_DISPLAY)
  {
    DrawSendingCoinsImage(coinAmount);
  }
}

void ProcessDisplayControl(int payloadSize)
{
  // First byte of the payload/serial buffer will indicate what to display
  switch(serialBuf[0])
  {
    case STRING_DISPLAY:
    {
      int yOffset = serialBuf[1];
      int stringLength = payloadSize - 2;
      char *extractedString = new char[stringLength + 1];
      for (int i = 0; i < stringLength; i++) 
      {
        extractedString[i] = (char)serialBuf[i+2];
      }
      extractedString[stringLength] = '\0';
      String displayString(extractedString);
      DisplayCustomStringMessage(displayString, yOffset);
      free(extractedString);
    }
    break;
    case LOGO_DISPLAY:
    DrawRadioDogeLogo();
    break;
    case DOGE_ANIMATION_DISPLAY:
    DogeAnimation();
    break;
    case COIN_ANIMATION_DISPLAY:
    CoinAnimation();
    break;
    case RECEIVING_DISPLAY:
    ProcessCoinDisplay(payloadSize);
    break;
    case SENDING_DISPLAY:
    ProcessCoinDisplay(payloadSize);
    break;
    default:
    // @TODO
    break;
  }
}

void SendHardwareInfoToHost()
{
  // Just send 'h' for heltec, board version, and firmware version for now
  uint8_t reply[5] = {HARDWARE_INFO, 3, 'h', HELTEC_BOARD_VERSION, FIRMWARE_VERSION}; 
  Serial.write(reply, 5);
}

void ParseHostFormedPacket(uint8_t payloadSize) {
  // Echo back information about what was read...
  char *extractedMessage = new char[payloadSize + 1];
  for (int i = 0; i < payloadSize; i++) {
    extractedMessage[i] = (char)serialBuf[i];
  }
  extractedMessage[payloadSize] = '\0';
  String messageString(extractedMessage);
  free(extractedMessage);
  debugPrintln(messageString);
}

// Parse a LoRa message received over the air from another module
// v0.4.0 (WP1) — Hand a received LoRa packet to the serial host so the app or the
// radiodoge-cli daemon can act on it. Desktop-format gateway packets (CMD_DOGE_TX
// 0x10, CMD_REQUEST_BALANCE 0x11) carry the app command byte and the same 8-byte
// header the host expects, so they are forwarded verbatim. Native MESSAGE packets
// (messageType MESSAGE == CMD_MESSAGE == 0x03) are already in the host's format,
// so relayed "TX_ACK:<txid>" / "BAL:<koinus>" replies reach the app too.
//
// Transaction/balance forwarding is limited to gateway mode (only a gateway host
// runs a daemon); incoming messages are always forwarded so the app sees replies.
void ForwardReceivedPacketToHost() {
  uint8_t cmd = rxPacket[0];
  bool forward = false;
  if (cmd == 0x10 || cmd == 0x11) {
    forward = gateway_mode;
  } else if (cmd == MESSAGE) {
    forward = true;
  }
  if (forward) {
    Serial.write(rxPacket, rxSize);
    addLog("[HOST] Forwarded LoRa cmd 0x" + String(cmd, HEX) + " to serial host (" + String(rxSize) + " bytes)");
  }
}

// v0.4.2 — Is this a desktop-protocol multipart frame (as opposed to the
// firmware's own 'm' multipart format)? Identified by the flags byte, since the
// command byte is the one the sequence carries, e.g. 0x10 CMD_DOGE_TX.
bool IsDesktopMultipartFrame() {
  if (rxSize < SINGLE_PACKET_HEADER_SIZE + DESKTOP_MULTIPART_EXTRA) {
    return false;
  }
  if ((rxPacket[1] & 0x0F) != DESKTOP_FLAG_MULTIPART) {
    return false;
  }
  uint8_t cmd = rxPacket[0];
  return cmd == 0x10 || cmd == 0x11 || cmd == 0x03;
}

void ParseReceivedMessage() {
  addLog("[DEBUG] Received packet - Size: " + String(rxSize) + " bytes, First byte: " + String(rxPacket[0]));

  // v0.4.2 — A desktop multipart frame is a fragment: only the host can put the
  // sequence back together, so hand it over untouched and stop. Running it
  // through the switch below would read the multipart header as message text
  // and print it to the OLED, and would never forward it at all.
  if (IsDesktopMultipartFrame()) {
    if (CheckIfPacketForMe() || CheckIfPacketIsGlobalBroadcast()) {
      uint8_t cmd   = (uint8_t)rxPacket[0];
      uint8_t total = (uint8_t)rxPacket[SINGLE_PACKET_HEADER_SIZE];
      uint8_t part  = (uint8_t)rxPacket[SINGLE_PACKET_HEADER_SIZE + 1] + 1;  // 0-based on the wire
      Serial.write(rxPacket, rxSize);
      SetSenderAddress();
      char partLabel[32];
      snprintf(partLabel, sizeof(partLabel), "Part %u/%u",
               (unsigned)part, (unsigned)total);
      DisplayRXMessage(String(partLabel), senderAddress);
      addLog("[HOST] Forwarded multipart cmd 0x" + String(cmd, HEX) +
             " part " + String(part) + "/" + String(total) +
             " to serial host (" + String(rxSize) + " bytes)");
    }
    return;
  }

  if (CheckIfPacketForMe()) {
    addLog("[DEBUG] Packet is for me - processing...");
    // v0.4.0 (WP1) — forward to the serial host (daemon/app) before local handling.
    ForwardReceivedPacketToHost();
    //Serial.println("PACKET FOR ME");
    messageType mType = (messageType)rxPacket[0];
    addLog("[DEBUG] Packet type: " + String(mType) + ", Is multipart: " + String(mType == MULTIPART_PACKET));
    // For multipart packets, addresses are in different positions
    if (mType == MULTIPART_PACKET) {
      addLog("[LoRa] Processing received packet - Type: " + String(mType) + " from " + String((int)rxPacket[4]) + "." + String((int)rxPacket[5]) + "." + String((int)rxPacket[6]) + " (Packet size: " + String(rxSize) + " bytes)");
    } else {
      addLog("[LoRa] Processing received packet - Type: " + String(mType) + " from " + String((int)rxPacket[2]) + "." + String((int)rxPacket[3]) + "." + String((int)rxPacket[4]) + " (Packet size: " + String(rxSize) + " bytes)");
    }
    
    // Debug: Log packet contents for multipart packets
    if (mType == MULTIPART_PACKET) {
      addLog("[DEBUG] Multipart packet - Part: " + String(rxPacket[7]) + "/" + String(rxPacket[8]) + ", DataType: " + String(rxPacket[9]));
      addLog("[DEBUG] Multipart addresses - To: " + String((int)rxPacket[1]) + "." + String((int)rxPacket[2]) + "." + String((int)rxPacket[3]) + ", From: " + String((int)rxPacket[4]) + "." + String((int)rxPacket[5]) + "." + String((int)rxPacket[6]));
      // Raw packet debugging
      String rawBytes = "";
      for (int i = 0; i < min(16, (int)rxSize); i++) {
        rawBytes += String(rxPacket[i], HEX) + " ";
      }
      addLog("[DEBUG] Raw packet bytes: " + rawBytes);
    }
    switch (mType) {
      case ACK:
        ReceivedACK();
        break;
      case PING:
        ReceivedPing();
        // Set the ACK destination as the address of the sender
        dest.region = rxPacket[2];
        dest.community = rxPacket[3];
        dest.node = rxPacket[4];
        // For some reason sending the ACK here works on V2 but not V3
        // Not sure why but sending it in the main loop works for both so we will do it there
        // We will just set a flag to send an ACK
        needToSendACK = true;
        break;
      case MESSAGE:
        {
          String messageString = ExtractStringMessageFromBuffer((uint8_t *)rxPacket, rxSize);
          SetSenderAddress();
          
          // Check if this is a Dogecoin node response
          if (messageString.startsWith("DOGECOIN_RESPONSE:")) {
            addLog("[LoRa] Received Dogecoin node response from " + String(senderAddress.region) + "." + String(senderAddress.community) + "." + String(senderAddress.node));
            addLog("[LoRa] Dogecoin response: " + messageString);
            addLog("[LoRa] Full Dogecoin node message received and displayed on screen");
            
            // Handle confirmation for queued requests
            HandleConfirmationReceived(messageString);
          }
          
          DisplayRXMessage(messageString, senderAddress);
          //Serial.printf("MUCH TLK FR %d.%d.%d:\n", senderAddress.region, senderAddress.community, senderAddress.node);
          //Serial.println(messageString);
        }
        break;
      case TRANSACTION:
        {
          String transactionString = ExtractStringMessageFromBuffer((uint8_t *)rxPacket, rxSize);
          SetSenderAddress();
          DisplayRXMessage("TX: " + transactionString, senderAddress);
          //Serial.printf("TRANSACTION FR %d.%d.%d:\n", senderAddress.region, senderAddress.community, senderAddress.node);
          //Serial.println(transactionString);
        }
        break;
      case BROADCAST:
        {
          String broadcastString = ExtractStringMessageFromBuffer((uint8_t *)rxPacket, rxSize);
          SetSenderAddress();
          DisplayBroadcastMessage(broadcastString, senderAddress);
          //Serial.printf("BROADCAST FR %d.%d.%d:\n", senderAddress.region, senderAddress.community, senderAddress.node);
          //Serial.println(broadcastString);
        }
        break;
      case HOST_FORMED_PACKET:
        SetSenderAddress();
        DisplayRXMessage("Packet Received!", senderAddress);
        // We will just pass on the message directly to the host
        Serial.write(rxPacket, rxSize);
        break;
      case MULTIPART_PACKET:
        SetSenderAddress();
        char tempBuf[64];
        sprintf(tempBuf, "Packet part %i of %i", rxPacket[7], rxPacket[8]);
        DisplayRXMessage(String(tempBuf), senderAddress);
        addLog("[LoRa] Processing MULTIPART packet - Part " + String(rxPacket[7]) + "/" + String(rxPacket[8]) + " from " + String(senderAddress.region) + "." + String(senderAddress.community) + "." + String(senderAddress.node));
        ProcessMultipartPacket();
        return; // Exit after processing multipart packet
        break;
      default:
        // (debug)
        //Serial.println("wat rcv?");
        break;
    }
  } 
  else if (CheckIfPacketIsGlobalBroadcast()){
    addLog("[DEBUG] Global broadcast packet received");
    messageType mType = (messageType)rxPacket[0];
    addLog("[DEBUG] Broadcast packet type: " + String(mType) + ", Is multipart: " + String(mType == MULTIPART_PACKET));
    // Check if it's a multipart broadcast
    if (rxPacket[0] == MULTIPART_PACKET) {
      addLog("[DEBUG] Multipart broadcast packet received");
      // Raw packet debugging for broadcast
      String rawBytes = "";
      for (int i = 0; i < min(16, (int)rxSize); i++) {
        rawBytes += String(rxPacket[i], HEX) + " ";
      }
      addLog("[DEBUG] Broadcast raw packet bytes: " + rawBytes);
      SetSenderAddress();
      char tempBuf[64];
      sprintf(tempBuf, "BC Part %i of %i", rxPacket[7], rxPacket[8]);
      DisplayBroadcastMessage(String(tempBuf), senderAddress);
      ProcessMultipartPacket();
      return; // Exit after processing multipart broadcast
    } else {
      // Regular broadcast
      SetSenderAddress();
      DisplayBroadcastMessage("Broadcast Received!", senderAddress);
      // Check broadcast type
      // @TODO
      // We will just pass on the broadcast message directly to the host
      Serial.write(rxPacket, rxSize);
    }
  }
  else {
    // For multipart packets, addresses are in different positions
    if (rxPacket[0] == MULTIPART_PACKET) {
      addLog("[DEBUG] Multipart packet not for me - From: " + String((int)rxPacket[4]) + "." + String((int)rxPacket[5]) + "." + String((int)rxPacket[6]) + ", To: " + String((int)rxPacket[1]) + "." + String((int)rxPacket[2]) + "." + String((int)rxPacket[3]));
      addLog("[DEBUG] Multipart broadcast check: " + String(CheckIfPacketIsGlobalBroadcast()));
    } else {
      addLog("[DEBUG] Packet not for me - From: " + String((int)rxPacket[2]) + "." + String((int)rxPacket[3]) + "." + String((int)rxPacket[4]) + ", To: " + String((int)rxPacket[5]) + "." + String((int)rxPacket[6]) + "." + String((int)rxPacket[7]));
    }
    //Serial.printf("NOT FR ME: FR %d.%d.%d\n", rxPacket[4], rxPacket[5], rxPacket[6]);
  }
}

// Checks to see if the received packet's destination is the same as the local address
bool CheckIfPacketForMe() {
  // For multipart packets, addresses are in different positions
  if (rxPacket[0] == MULTIPART_PACKET) {
    // Multipart packet: [type][dest_region][dest_community][dest_node][src_region][src_community][src_node]...
    return (local.region == rxPacket[1]) && (local.community == rxPacket[2]) && (local.node == rxPacket[3]);
  } else {
    // Regular packet: [type][header][src_region][src_community][src_node][dest_region][dest_community][dest_node]...
    return (local.region == rxPacket[5]) && (local.community == rxPacket[6]) && (local.node == rxPacket[7]);
  }
}

// Checks to see if the received packet's destination is intended for every listening node
bool CheckIfPacketIsGlobalBroadcast()
{
  // For multipart packets, addresses are in different positions
  if (rxPacket[0] == MULTIPART_PACKET) {
    // Multipart packet: [type][dest_region][dest_community][dest_node][src_region][src_community][src_node]...
    return (255 == rxPacket[1]) && (255 == rxPacket[2]) && (255 == rxPacket[3]);
  } else {
    // Regular packet: [type][header][src_region][src_community][src_node][dest_region][dest_community][dest_node]...
    return (255 == rxPacket[5]) && (255 == rxPacket[6]) && (255 == rxPacket[7]);
  }
}

// Extract the payload message from the given buffer (assists with displaying on screen)
String ExtractStringMessageFromBuffer(uint8_t *buf, int bufferSize) {
  // v0.4.x fix — the previous implementation paired new[] with free() (undefined
  // behavior), read one byte past the end of the buffer, and started at offset 7
  // (one byte before the payload), prepending the destination-node byte as a
  // stray leading character. The 8-byte packet header is [type, flags, src(3),
  // dst(3)] and the payload begins at offset 8. Build the String directly from
  // offset 8, stopping at the payload end or the first NUL, whichever comes first.
  String messageString = "";
  if (bufferSize <= SINGLE_PACKET_HEADER_SIZE) {
    return messageString;
  }
  int messageSize = bufferSize - SINGLE_PACKET_HEADER_SIZE;
  messageString.reserve(messageSize);
  for (int i = 0; i < messageSize; i++) {
    char c = (char)buf[SINGLE_PACKET_HEADER_SIZE + i];
    if (c == '\0') {
      break;
    }
    messageString += c;
  }
  return messageString;
}

// Web Server Functions
void setupWebServer() {
  // Serve the main control page
  server.on("/", handleRoot);
  
  // API endpoints (legacy text format)
  server.on("/ping", handlePing);
  server.on("/message", handleMessage);
  server.on("/transaction", handleTransaction);
  server.on("/broadcast", handleBroadcast);
  server.on("/ack", handleACK);
  server.on("/address", handleAddress);
  server.on("/status", handleStatus);
  
  // REST API endpoints with JSON support
  server.on("/api/ping", HTTP_GET, handleApiPing);
  server.on("/api/ping", HTTP_POST, handleApiPing);
  server.on("/api/message", HTTP_GET, handleApiMessage);
  server.on("/api/message", HTTP_POST, handleApiMessage);
  server.on("/api/transaction", HTTP_GET, handleApiTransaction);
  server.on("/api/transaction", HTTP_POST, handleApiTransaction);
  server.on("/api/broadcast", HTTP_GET, handleApiBroadcast);
  server.on("/api/broadcast", HTTP_POST, handleApiBroadcast);
  server.on("/api/status", HTTP_GET, handleApiStatus);
  server.on("/api/address", HTTP_GET, handleApiAddress);
  server.on("/api/address", HTTP_POST, handleApiAddress);
  
  // WiFi Configuration endpoints
  server.on("/api/wifi", HTTP_GET, handleApiWifi);
  server.on("/api/wifi", HTTP_POST, handleApiWifi);
  server.on("/api/wifi/connect", HTTP_POST, handleApiWifiConnect);
  server.on("/api/wifi/disconnect", HTTP_POST, handleApiWifiDisconnect);
  server.on("/api/wifi/clear", HTTP_POST, handleApiWifiClear);
  
  // Internet Bridge endpoints
  server.on("/api/bridge/enable", HTTP_POST, handleApiBridgeEnable);
  server.on("/api/bridge/disable", HTTP_POST, handleApiBridgeDisable);
  server.on("/api/bridge/status", HTTP_GET, handleApiBridgeStatus);
  server.on("/proxy", HTTP_GET, handleHttpProxy);
  server.on("/api/lora/clear", HTTP_POST, handleApiLoRaClear);
  server.on("/api/password/change", HTTP_POST, handleApiPasswordChange);
  server.on("/api/password/reset", HTTP_POST, handleApiPasswordReset);
  server.on("/api/password/status", HTTP_GET, handleApiPasswordStatus);
  server.on("/api/gateway", HTTP_POST, handleApiGateway);
  server.on("/api/gateway/status", HTTP_GET, handleApiGatewayStatus);
  server.on("/api/gateway/save", HTTP_POST, handleApiGatewaySave);
  server.on("/api/gateway/clear", HTTP_POST, handleApiGatewayClear);
  server.on("/api/gateway/test", HTTP_POST, handleApiGatewayTest);
  server.on("/api/gateway/debug", HTTP_GET, handleApiGatewayDebug);
  server.on("/api/gateway/load", HTTP_GET, handleApiGatewayLoad);
  server.on("/api/logs", HTTP_GET, handleApiLogs);
  server.on("/api/logs/text", HTTP_GET, handleApiLogsText);
  server.on("/api/logs/send", HTTP_POST, handleApiLogsSend);
  server.on("/api/transaction/send", HTTP_POST, handleApiTransactionSend);
  server.on("/api/gateway/config", HTTP_GET, handleApiGatewayConfig);
  server.on("/api/gateway/config", HTTP_POST, handleApiGatewayConfigSet);
  server.on("/api/rpc", HTTP_POST, handleApiRpc);
  server.on("/api/jsonrpc", HTTP_POST, handleApiJsonRpc);
  server.on("/api/multipart/status", HTTP_GET, handleApiMultipartStatus);
  server.on("/api/queue/status", HTTP_GET, handleApiQueueStatus);
  
  server.begin();
  addLog("[WiFi] Web server started");
}

void handleRoot() {
  String html = "<!DOCTYPE html><html><head><title>RadioDoge Control</title>";
  html += "<meta name='viewport' content='width=device-width, initial-scale=1'>";
  html += "<style>";
  html += "body{font-family:'Comic Sans MS','Segoe UI',Tahoma,Geneva,Verdana,sans-serif;margin:0;background:#121212;color:#ffffff;min-height:100vh;line-height:1.6;}";
  html += ".container{max-width:900px;margin:0 auto;background:#1a1a1a;padding:30px;border-radius:20px;box-shadow:0 20px 40px rgba(0,0,0,0.5);margin-top:20px;margin-bottom:20px;}";
  html += "h1{color:#ffc107;text-align:center;margin-bottom:40px;font-size:3em;font-weight:700;text-shadow:0 0 20px rgba(255,193,7,0.3);letter-spacing:2px;display:flex;align-items:center;justify-content:center;gap:20px;}";
  html += ".title-icon{width:60px;height:60px;fill:#ffc107;filter:drop-shadow(0 0 10px rgba(255,193,7,0.5));}";
  html += ".signal-wave{animation:transmit 2s ease-in-out infinite;}";
  html += ".signal-wave:nth-child(2){animation-delay:0.3s;}";
  html += ".signal-wave:nth-child(3){animation-delay:0.6s;}";
  html += ".signal-wave:nth-child(4){animation-delay:0.9s;}";
  html += "@keyframes transmit{0%,100%{opacity:0;transform:scale(0.5);}50%{opacity:1;transform:scale(1);}}";
  html += "h2{color:#ffc107;border-bottom:3px solid #ffc107;padding-bottom:15px;margin-top:40px;font-size:1.8em;font-weight:600;text-transform:uppercase;letter-spacing:1px;}";
  html += "h3{color:#ffc107;margin:20px 0 15px 0;font-size:1.3em;font-weight:600;}";
  html += ".button{background:linear-gradient(135deg,#ffc107 0%,#ffb300 100%);color:#121212;border:none;padding:15px 30px;margin:8px 0;border-radius:12px;cursor:pointer;font-size:16px;font-weight:700;transition:all 0.3s ease;text-transform:uppercase;letter-spacing:1px;box-shadow:0 4px 15px rgba(255,193,7,0.3);width:100%;box-sizing:border-box;}";
  html += ".button:hover{background:linear-gradient(135deg,#ffb300 0%,#ffa000 100%);transform:translateY(-3px);box-shadow:0 8px 25px rgba(255,193,7,0.4);}";
  html += ".button:active{transform:translateY(-1px);}";
  html += ".button.danger{background:linear-gradient(135deg,#6c757d 0%,#5a6268 100%);box-shadow:0 4px 15px rgba(108,117,125,0.3);color:#ffffff;}";
  html += ".button.danger:hover{background:linear-gradient(135deg,#5a6268 0%,#495057 100%);box-shadow:0 8px 25px rgba(108,117,125,0.4);}";
  html += ".button.success{background:linear-gradient(135deg,#28a745 0%,#218838 100%);box-shadow:0 4px 15px rgba(40,167,69,0.3);color:#ffffff;}";
  html += ".button.success:hover{background:linear-gradient(135deg,#218838 0%,#1e7e34 100%);box-shadow:0 8px 25px rgba(40,167,69,0.4);}";
  html += "input[type=text],input[type=number],input[type=password],textarea,select{width:100%;padding:15px;margin:8px 0;border:2px solid #333;border-radius:12px;font-size:16px;box-sizing:border-box;background:#2a2a2a;color:#ffffff;transition:all 0.3s ease;}";
  html += "input[type=text]:focus,input[type=number]:focus,input[type=password]:focus,textarea:focus,select:focus{border-color:#ffc107;outline:none;box-shadow:0 0 15px rgba(255,193,7,0.2);background:#333;}";
  html += "input[type=text]::placeholder,input[type=number]::placeholder,input[type=password]::placeholder,textarea::placeholder{color:#888;}";
  html += ".status{background:linear-gradient(135deg,#2a2a2a 0%,#333 100%);padding:20px;border-radius:15px;margin:20px 0;border-left:5px solid #ffc107;box-shadow:0 5px 15px rgba(0,0,0,0.3);}";
  html += ".section{background:linear-gradient(135deg,#1e1e1e 0%,#2a2a2a 100%);padding:25px;margin:20px 0;border-radius:15px;border:1px solid #333;box-shadow:0 5px 15px rgba(0,0,0,0.2);transition:all 0.3s ease;}";
  html += ".section:hover{transform:translateY(-2px);box-shadow:0 8px 25px rgba(0,0,0,0.3);}";
  html += ".grid{display:grid;grid-template-columns:1fr 1fr;gap:25px;margin:25px 0;}";
  html += ".response{background:#1a1a1a;padding:20px;border-radius:12px;margin:20px 0;border-left:5px solid #ffc107;font-family:'Courier New',monospace;white-space:pre-wrap;color:#ffc107;box-shadow:0 5px 15px rgba(0,0,0,0.3);}";
  html += ".address-display{background:linear-gradient(135deg,#ffc107 0%,#ffb300 100%);color:#121212;padding:5px;border-radius:10px;margin:10px 0;font-weight:700;text-align:center;box-shadow:0 4px 15px rgba(255,193,7,0.3);}";
  html += "label{color:#ffc107;font-weight:600;margin-bottom:8px;display:block;text-transform:uppercase;letter-spacing:1px;font-size:0.9em;}";
  html += "p{color:#ccc;margin:10px 0;}";
  html += ".button-group{margin:15px 0;}";
  html += ".accordion{background:linear-gradient(135deg,#1e1e1e 0%,#2a2a2a 100%);border:1px solid #333;border-radius:15px;margin:20px 0;overflow:hidden;box-shadow:0 5px 15px rgba(0,0,0,0.2);}";
  html += ".accordion-header{background:linear-gradient(135deg,#ffc107 0%,#ffb300 100%);color:#121212;padding:20px 25px;cursor:pointer;font-size:1.4em;font-weight:700;text-transform:uppercase;letter-spacing:1px;transition:all 0.3s ease;display:flex;justify-content:space-between;align-items:center;}";
  html += ".accordion-header:hover{background:linear-gradient(135deg,#ffb300 0%,#ffa000 100%);}";
  html += ".accordion-header.active{background:linear-gradient(135deg,#ffa000 0%,#ff8f00 100%);}";
  html += ".accordion-content{padding:0;max-height:0;overflow:hidden;transition:max-height 0.3s ease;background:#1a1a1a;}";
  html += ".accordion-content.active{max-height:5000px;padding:25px;overflow-y:auto;}";
  html += ".accordion-icon{width:20px;height:20px;transition:transform 0.3s ease;fill:#121212;}";
  html += ".accordion-icon.rotated{transform:rotate(180deg);}";
  html += ".section-header-gray{background:linear-gradient(135deg,#6c757d 0%,#5a6268 100%);color:#ffffff;}";
  html += ".section-header-gray:hover{background:linear-gradient(135deg,#5a6268 0%,#495057 100%);}";
  html += ".section-header-gray.active{background:linear-gradient(135deg,#495057 0%,#343a40 100%);}";
  html += ".section-header-gray .accordion-icon{fill:#ffffff;}";
  html += ".status{background:linear-gradient(135deg,#2a2a2a 0%,#333 100%);padding:20px;border-radius:15px;margin:20px 0;border-left:5px solid #ffc107;box-shadow:0 5px 15px rgba(0,0,0,0.3);text-align:center;}";
  html += ".logs-container{background:#1a1a1a;border:1px solid #333;border-radius:8px;padding:15px;max-height:400px;overflow-y:auto;font-family:monospace;font-size:12px;margin-top:15px;}";
  html += ".log-entry{padding:5px 0;border-bottom:1px solid #333;color:#ccc;word-wrap:break-word;}";
  html += ".log-entry:last-child{border-bottom:none;}";
  html += ".log-entry.error{color:#ff6b6b;}";
  html += ".log-entry.warning{color:#ffd93d;}";
  html += ".log-entry.info{color:#6bcf7f;}";
  html += ".log-entry.debug{color:#4dabf7;}";
  html += "@media (max-width: 768px) {.grid{grid-template-columns:1fr;} .container{padding:20px;margin:10px;} h1{font-size:2.2em;flex-direction:column;gap:10px;} .title-icon{width:50px;height:50px;} .button{padding:12px 20px;font-size:14px;} .accordion-header{font-size:1.2em;padding:15px 20px;}}";
  html += "</style></head><body>";
  
  html += "<div class='container'>";
  html += "<h1>";
  html += "<svg class='title-icon' viewBox='0 0 100 100'>";
  html += "<defs>";
  html += "<linearGradient id='towerGradient' x1='0%' y1='0%' x2='100%' y2='100%'>";
  html += "<stop offset='0%' style='stop-color:#ffc107;stop-opacity:1' />";
  html += "<stop offset='100%' style='stop-color:#ffb300;stop-opacity:1' />";
  html += "</linearGradient>";
  html += "</defs>";
  html += "<!-- Radio Tower Base -->";
  html += "<rect x='45' y='70' width='10' height='25' fill='url(#towerGradient)' rx='2'/>";
  html += "<!-- Tower Mast -->";
  html += "<rect x='48' y='20' width='4' height='50' fill='url(#towerGradient)' rx='1'/>";
  html += "<!-- Antenna Array -->";
  html += "<rect x='35' y='15' width='30' height='3' fill='url(#towerGradient)' rx='1'/>";
  html += "<rect x='40' y='12' width='20' height='3' fill='url(#towerGradient)' rx='1'/>";
  html += "<rect x='45' y='9' width='10' height='3' fill='url(#towerGradient)' rx='1'/>";
  html += "<!-- Center Antenna -->";
  html += "<rect x='49' y='5' width='2' height='10' fill='url(#towerGradient)' rx='1'/>";
  html += "<!-- Transmission Signals -->";
  html += "<circle cx='50' cy='50' r='15' stroke='#ffc107' stroke-width='2' fill='none' opacity='0.3' class='signal-wave'/>";
  html += "<circle cx='50' cy='50' r='25' stroke='#ffc107' stroke-width='2' fill='none' opacity='0.2' class='signal-wave'/>";
  html += "<circle cx='50' cy='50' r='35' stroke='#ffc107' stroke-width='2' fill='none' opacity='0.1' class='signal-wave'/>";
  html += "<circle cx='50' cy='50' r='45' stroke='#ffc107' stroke-width='2' fill='none' opacity='0.05' class='signal-wave'/>";
  html += "</svg>";
  html += "RadioDoge";
  html += "</h1>";
  
  // Device Status
  html += "<div class='status'>";
  html += "<h3>DEVICE STATUS</h3>";
  html += "<p><strong>Address:</strong> <span class='address-display'>" + String(local.region) + "." + String(local.community) + "." + String(local.node) + "</span></p>";
  html += "<p><strong>LoRa:</strong> Active | <strong>WiFi AP:</strong> " + String(ap_ssid) ;
  if (internet_connected) {
    html += "<p><strong>Internet:</strong> <span style='color:#28a745;'>Online</span> | <strong>IP:</strong> " + WiFi.localIP().toString() + "</p>";
  } else {
    html += "<p><strong>Internet:</strong> <span style='color:#888;'>Offline</span></p>";
  }
  html += "</div>";
  
  
  // Transaction Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header' onclick='toggleAccordion(\"transaction\")'>";
  html += "<span>DOGECOIN TRANSACTION</span>";
  html += "<svg class='accordion-icon' id='transaction-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='transaction-content'>";
  html += "<p>Send signed Dogecoin transaction via LoRa</p>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Target Address</label>";
  html += "<input type='text' id='txAddress' placeholder='10.1.2' value='10.1.2'>";
  html += "</div>";
  html += "<div>";
  html += "<label>Transaction Type</label>";
  html += "<select id='txType'>";
  html += "<option value='signed'>Signed Transaction</option>";
  html += "<option value='raw'>Raw Transaction</option>";
  html += "<option value='utxo'>UTXO Data</option>";
  html += "</select>";
  html += "</div>";
  html += "</div>";
  html += "<textarea id='transaction' placeholder='Paste your signed Dogecoin transaction here...' rows='4'></textarea>";
  html += "<div class='button-group'>";
  html += "<button class='button success' onclick='sendTransaction()'>SEND TRANSACTION</button>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  
  // Broadcast Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header' onclick='toggleAccordion(\"broadcast\")'>";
  html += "<span>BROADCAST MESSAGE</span>";
  html += "<svg class='accordion-icon' id='broadcast-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='broadcast-content'>";
  html += "<p>Send message to all devices in range</p>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Broadcast Type</label>";
  html += "<select id='broadcastType'>";
  html += "<option value='announcement'>Announcement</option>";
  html += "<option value='emergency'>Emergency</option>";
  html += "<option value='network'>Network Update</option>";
  html += "<option value='transaction'>Transaction Broadcast</option>";
  html += "</select>";
  html += "</div>";
  html += "<div>";
  html += "<label>Priority</label>";
  html += "<select id='broadcastPriority'>";
  html += "<option value='normal'>Normal</option>";
  html += "<option value='high'>High</option>";
  html += "<option value='urgent'>Urgent</option>";
  html += "</select>";
  html += "</div>";
  html += "</div>";
  html += "<textarea id='broadcastMessage' placeholder='Enter broadcast message...' rows='3'></textarea>";
  html += "<div class='button-group'>";
  html += "<button class='button' onclick='sendBroadcast()'>BROADCAST TO ALL</button>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  
  // ACK Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header' onclick='toggleAccordion(\"ack\")'>";
  html += "<span>ACKNOWLEDGMENT</span>";
  html += "<svg class='accordion-icon' id='ack-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='ack-content'>";
  html += "<p>Send ACK for received messages</p>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>ACK Target</label>";
  html += "<input type='text' id='ackAddress' placeholder='10.1.2' value='10.1.2'>";
  html += "</div>";
  html += "<div>";
  html += "<label>ACK Type</label>";
  html += "<select id='ackType'>";
  html += "<option value='received'>Message Received</option>";
  html += "<option value='processed'>Message Processed</option>";
  html += "<option value='error'>Error Response</option>";
  html += "</select>";
  html += "</div>";
  html += "</div>";
  html += "<div class='button-group'>";
  html += "<button class='button' onclick='sendACK()'>SEND ACK</button>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
   
   // Message Section
   html += "<div class='accordion'>";
   html += "<div class='accordion-header' onclick='toggleAccordion(\"message\")'>";
   html += "<span>SEND MESSAGE</span>";
   html += "<svg class='accordion-icon' id='message-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
   html += "</div>";
   html += "<div class='accordion-content' id='message-content'>";
   html += "<p>Send text/data transmission via LoRa</p>";
   html += "<div class='grid'>";
   html += "<div>";
   html += "<label>Target Address</label>";
   html += "<input type='text' id='msgAddress' placeholder='10.1.2' value='10.1.2'>";
   html += "</div>";
   html += "<div>";
   html += "<label>Message Type</label>";
   html += "<select id='msgType'>";
   html += "<option value='text'>Text Message</option>";
   html += "<option value='data'>Data Packet</option>";
   html += "<option value='command'>Command</option>";
   html += "</select>";
   html += "</div>";
   html += "</div>";
   html += "<textarea id='message' placeholder='Enter your message here...' rows='3'></textarea>";
   html += "<div class='button-group'>";
   html += "<button class='button' onclick='sendMessage()'>SEND MESSAGE</button>";
   html += "</div>";
   html += "</div>";
   html += "</div>";
  
  // Device Configuration
  html += "<div class='accordion'>";
  html += "<div class='accordion-header section-header-gray' onclick='toggleAccordion(\"config\")'>";
  html += "<span>DEVICE CONFIGURATION</span>";
  html += "<svg class='accordion-icon' id='config-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='config-content'>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Region (0-255)</label>";
  html += "<input type='number' id='region' placeholder='10' min='0' max='255' value='" + String(local.region) + "'>";
  html += "</div>";
  html += "<div>";
  html += "<label>Community (0-255)</label>";
  html += "<input type='number' id='community' placeholder='1' min='0' max='255' value='" + String(local.community) + "'>";
  html += "</div>";
  html += "</div>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Node (0-255)</label>";
  html += "<input type='number' id='node' placeholder='1' min='0' max='255' value='" + String(local.node) + "'>";
  html += "</div>";
  html += "<div class='button-group'>";
  html += "<button class='button' onclick='setAddress()'>SET ADDRESS</button>";
  html += "<button class='button' onclick='getStatus()'>GET STATUS</button>";
  html += "<button class='button' onclick='getCurrentAddress()'>CURRENT ADDRESS</button>";
  html += "<button class='button' onclick='getMultipartStatus()'>MULTIPART STATUS</button>";
  html += "<button class='button' onclick='getDetailedLogs()'>DETAILED LOGS</button>";
  html += "<button class='button danger' onclick='clearLoRaConfig()'>CLEAR STORED CONFIG</button>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  html += "</div>";

  // Ping Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header section-header-gray' onclick='toggleAccordion(\"ping\")'>";
  html += "<span>PING TEST</span>";
  html += "<svg class='accordion-icon' id='ping-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='ping-content'>";
  html += "<p>Test connectivity between devices</p>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Target Region</label>";
  html += "<input type='number' id='pingRegion' placeholder='10' min='0' max='255' value='10'>";
  html += "</div>";
  html += "<div>";
  html += "<label>Target Community</label>";
  html += "<input type='number' id='pingCommunity' placeholder='1' min='0' max='255' value='1'>";
  html += "</div>";
  html += "</div>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Target Node</label>";
  html += "<input type='number' id='pingNode' placeholder='2' min='0' max='255' value='2'>";
  html += "</div>";
  html += "<div class='button-group'>";
  html += "<button class='button' onclick='sendPing()'>SEND PING</button>";
  html += "<button class='button' onclick='pingAll()'>PING ALL</button>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  html += "</div>";  
  
  // WiFi Configuration Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header section-header-gray' onclick='toggleAccordion(\"wifi\")'>";
  html += "<span>WIFI CONFIGURATION</span>";
  html += "<svg class='accordion-icon' id='wifi-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='wifi-content'>";
  html += "<p>Configure internet WiFi connection for transaction forwarding</p>";
  html += "<form>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>WiFi SSID</label>";
  html += "<input type='text' id='wifiSSID' placeholder='Your WiFi Network Name' value='" + internet_ssid + "' autocomplete='username'>";
  html += "</div>";
  html += "<div>";
  html += "<label>WiFi Password</label>";
  html += "<input type='password' id='wifiPassword' placeholder='Your WiFi Password' value='" + internet_password + "' autocomplete='current-password'>";
  html += "</div>";
  html += "</div>";
  html += "</form>";
  html += "<div class='button-group'>";
  html += "<button class='button success' onclick='connectWiFi()'>CONNECT TO INTERNET</button>";
  html += "<button class='button danger' onclick='disconnectWiFi()'>DISCONNECT</button>";
  html += "<button class='button' onclick='getWiFiStatus()'>REFRESH STATUS</button>";
  html += "<button class='button danger' onclick='clearWiFiCredentials()'>CLEAR STORED CREDENTIALS</button>";
  html += "</div>";
  html += "<div id='wifiStatus' class='response' style='display:none;'></div>";
  html += "</div>";
  html += "</div>";
  
  // Internet Bridge Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header section-header-gray' onclick='toggleAccordion(\"bridge\")'>";
  html += "<span>INTERNET BRIDGE</span>";
  html += "<svg class='accordion-icon' id='bridge-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='bridge-content'>";
  html += "<p><strong>Share Internet Connection:</strong> Enable internet access for devices connected to the RadioDoge WiFi network</p>";
  html += "<div class='status'>";
  html += "<h3>Bridge Status</h3>";
  html += "<p><strong>Internet Connection:</strong> <span id='bridgeInternetStatus'>" + String(internet_connected ? "Connected" : "Not Connected") + "</span></p>";
  html += "<p><strong>Bridge Status:</strong> <span id='bridgeStatus'>" + String(internet_bridge_enabled ? "Enabled" : "Disabled") + "</span></p>";
  html += "<p><strong>AP Gateway:</strong> " + ap_gateway.toString() + "</p>";
  html += "<p><strong>Internet IP:</strong> <span id='bridgeInternetIP'>" + (internet_connected ? WiFi.localIP().toString() : "None") + "</span></p>";
  html += "</div>";
  html += "<div class='button-group'>";
  html += "<button class='button success' onclick='enableBridge()' id='enableBridgeBtn'>ENABLE BRIDGE</button>";
  html += "<button class='button danger' onclick='disableBridge()' id='disableBridgeBtn'>DISABLE BRIDGE</button>";
  html += "<button class='button' onclick='getBridgeStatus()'>REFRESH STATUS</button>";
  html += "</div>";
  html += "<div id='bridgeStatus' class='response' style='display:none;'></div>";
  html += "<div class='section'>";
  html += "<h3>Internet Bridge Status</h3>";
  html += "<p><strong>Note:</strong> ESP32 has limited NAT capabilities. For full internet access, use the HTTP proxy below.</p>";
  html += "<div class='button-group'>";
  html += "<button class='button' onclick='testProxy()'>TEST PROXY</button>";
  html += "<button class='button' onclick='openGoogle()'>OPEN GOOGLE</button>";
  html += "</div>";
  html += "<div id='proxyStatus' class='response' style='display:none;'></div>";
  html += "</div>";
  html += "<div class='section'>";
  html += "<h3>How to Access Internet</h3>";
  html += "<p><strong>Method 1: HTTP Proxy (Recommended)</strong></p>";
  html += "<p>Use the RadioDoge device as an HTTP proxy to access websites:</p>";
  html += "<ul style='margin:10px 0;padding-left:20px;'>";
  html += "<li><strong>Format:</strong> <code>http://192.168.4.1/proxy?url=WEBSITE_URL</code></li>";
  html += "<li><strong>Example:</strong> <code>http://192.168.4.1/proxy?url=google.com</code></li>";
  html += "<li><strong>Example:</strong> <code>http://192.168.4.1/proxy?url=https://github.com</code></li>";
  html += "</ul>";
  html += "<p><strong>Method 2: Direct API Access</strong></p>";
  html += "<p>Use the RadioDoge API to access internet services programmatically.</p>";
  html += "<p><strong>Requirements:</strong> The RadioDoge device must be connected to an internet WiFi network</p>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  
  // AP Password Management Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header section-header-gray' onclick='toggleAccordion(\"password\")'>";
  html += "<span>ACCESS POINT</span>";
  html += "<svg class='accordion-icon' id='password-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='password-content'>";
  html += "<p>Change the RadioDoge WiFi access point password for enhanced security</p>";
  html += "<form>";
  html += "<div class='grid'>";
  html += "<div>";
  // v0.4.2 — The current password used to be rendered here as the field's value.
  // `type='password'` only masks it on screen: it sat in cleartext in the served
  // HTML, readable with a plain GET by anyone who can reach the web server —
  // which in dual-WiFi mode includes every host on the upstream LAN, not just
  // devices that already joined the AP. It also defeated the point of the
  // v0.4.1 change that stopped /api/password/status returning it.
  //
  // Nothing read this field: no script referenced it and handleApiPasswordChange
  // never verified a current password. It existed only to display the secret, so
  // it is gone rather than blanked.
  html += "<label>Current Password</label>";
  html += "<input type='password' placeholder='(not shown)' value='' disabled autocomplete='off'>";
  html += "</div>";
  html += "<div>";
  html += "<label>New Password</label>";
  html += "<input type='password' id='newPassword' placeholder='Enter new password (8-32 chars, letters + numbers)' autocomplete='new-password'>";
  html += "</div>";
  html += "</div>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Confirm New Password</label>";
  html += "<input type='password' id='confirmPassword' placeholder='Confirm new password' autocomplete='new-password'>";
  html += "</div>";
  html += "</form>";
  html += "<div>";
  html += "<label>Password Requirements</label>";
  html += "<div style='background:#1a1a1a;padding:10px;border-radius:8px;font-size:0.9em;'>";
  html += "<p><svg width='12' height='12' viewBox='0 0 24 24' fill='#28a745' style='vertical-align:middle;margin-right:8px;'><path d='M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z'/></svg>8-32 characters long</p>";
  html += "<p><svg width='12' height='12' viewBox='0 0 24 24' fill='#28a745' style='vertical-align:middle;margin-right:8px;'><path d='M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z'/></svg>Must contain letters and numbers</p>";
  html += "<p><svg width='12' height='12' viewBox='0 0 24 24' fill='#28a745' style='vertical-align:middle;margin-right:8px;'><path d='M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z'/></svg>Special characters allowed</p>";
  html += "<p><svg width='12' height='12' viewBox='0 0 24 24' fill='#28a745' style='vertical-align:middle;margin-right:8px;'><path d='M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z'/></svg>Case sensitive</p>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  html += "<div class='button-group'>";
  html += "<button class='button success' onclick='changePassword()'>CHANGE PASSWORD</button>";
  html += "<button class='button danger' onclick='resetPassword()'>RESET TO DEFAULT</button>";
  html += "<button class='button' onclick='getPasswordStatus()'>REFRESH STATUS</button>";
  html += "</div>";
  html += "<div id='passwordStatus' class='response' style='display:none;'></div>";
  html += "</div>";
  html += "</div>";
  
  // Doge Internet Gateway Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header section-header-gray' onclick='toggleAccordion(\"gateway\")'>";
  html += "<span>DOGE INTERNET GATEWAY</span>";
  html += "<svg class='accordion-icon' id='gateway-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='gateway-content'>";
  html += "<p>Send Dogecoin transactions to internet gateways</p>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Gateway Type</label>";
  html += "<select id='gatewayType' onchange='updateGatewayFields()'>";
  html += "<option value='none'>None</option>";
  html += "<option value='core'>CORE</option>";
  html += "<option value='dogebox'>DogeBox</option>";
  html += "<option value='wallet'>Dogecoin Wallet</option>";
  html += "<option value='custom'>Custom</option>";
  html += "</select>";
  html += "</div>";
  html += "<div id='ipField'>";
  html += "<label>IP Address</label>";
  html += "<input type='text' id='gatewayIp' placeholder='192.168.1.100'>";
  html += "</div>";
  html += "</div>";
  html += "<div id='rpcFields' style='display:none;'>";
  html += "<form>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>RPC Username</label>";
  html += "<input type='text' id='rpcUsername' placeholder='rpcuser' autocomplete='username'>";
  html += "</div>";
  html += "<div>";
  html += "<label>RPC Password</label>";
  html += "<input type='password' id='rpcPassword' placeholder='rpcpassword' autocomplete='current-password'>";
  html += "</div>";
  html += "</div>";
  html += "</form>";
  html += "</div>";
  html += "<div id='customFields' style='display:none;'>";
  html += "<div class='grid'>";
  html += "<div>";
  html += "<label>Port</label>";
  html += "<input type='text' id='gatewayPort' placeholder='8080'>";
  html += "</div>";
  html += "<div id='endpointField'>";
  html += "<label>Endpoint</label>";
  html += "<input type='text' id='gatewayEndpoint' placeholder='/api/push/tx'>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  html += "<div id='gatewayInfo' style='display:none;'>";
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;border-left:4px solid #ffc107;'>";
  html += "<h4 style='color:#ffc107;margin:0 0 10px 0;'>Gateway Information</h4>";
  html += "<p id='gatewayDescription' style='margin:5px 0;color:#ccc;'></p>";
  html += "<p id='gatewayEndpoint' style='margin:5px 0;color:#ffc107;font-family:monospace;'></p>";
  html += "<p id='gatewayRequirements' style='margin:5px 0;color:#888;font-size:0.9em;'></p>";
  html += "</div>";
  html += "</div>";
  html += "<div>";
  html += "<label>Transaction Data</label>";
  html += "<textarea id='gatewayTransaction' placeholder='Paste signed Dogecoin transaction here...' rows='4'></textarea>";
  html += "</div>";
  html += "<div class='button-group'>";
  html += "<button class='button success' onclick='sendToGateway()'>SEND TO INTERNET</button>";
  html += "<button class='button' onclick='testGateway()'>TEST CONNECTION</button>";
  html += "<button class='button' onclick='saveGatewayCredentials()'>SAVE CREDENTIALS</button>";
  html += "<button class='button danger' onclick='clearGatewayCredentials()'>CLEAR CREDENTIALS</button>";
  html += "</div>";
  html += "<div id='gatewayStatus' class='response' style='display:none;'></div>";
  html += "</div>";
  html += "</div>";
  
  // Real-Time Logs Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header section-header-gray' onclick='toggleAccordion(\"logs\")'>";
  html += "<span>REAL-TIME LOGS</span>";
  html += "<svg class='accordion-icon' id='logs-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div id='logs-content' class='accordion-content'>";
  html += "<div class='section'>";
  html += "<h3>System Logs</h3>";
  html += "<p>Monitor all RadioDoge system logs including display messages, network activity, and transaction processing.</p>";
  html += "<div class='button-group'>";
  html += "<button class='button' onclick='refreshLogs()'>REFRESH LOGS</button>";
  html += "<button class='button' onclick='clearLogs()'>CLEAR LOGS</button>";
  html += "<button class='button' onclick='toggleAutoRefresh()' id='autoRefreshBtn'>AUTO REFRESH: OFF</button>";
  html += "</div>";
  html += "<div id='logsContainer' class='logs-container'>";
  html += "<div class='log-entry'>Loading logs...</div>";
  html += "</div>";
  html += "<div class='section' style='margin-top:20px;'>";
  html += "<h4>Send Logs to Other Devices</h4>";
  html += "<p>Send current logs to another RadioDoge device via LoRa for remote monitoring.</p>";
  html += "<div class='form-group'>";
  html += "<label>Target Address (region.community.node)</label>";
  html += "<input type='text' id='logAddress' placeholder='10.1.2' value='10.1.2'>";
  html += "</div>";
  html += "<div class='form-group'>";
  html += "<label>Log Type</label>";
  html += "<select id='logType'>";
  html += "<option value='logs'>System Logs</option>";
  html += "<option value='debug'>Debug Info</option>";
  html += "<option value='error'>Error Logs</option>";
  html += "<option value='all'>All Logs</option>";
  html += "</select>";
  html += "</div>";
  html += "<button class='button success' onclick='sendLogs()'>SEND LOGS VIA LORA</button>";
  html += "<div id='logSendStatus' class='response' style='display:none;'></div>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  html += "</div>";
  
  // User Guide Section
  html += "<div class='accordion'>";
  html += "<div class='accordion-header' onclick='toggleAccordion(\"guide\")'>";
  html += "<span>USER GUIDE</span>";
  html += "<svg class='accordion-icon' id='guide-icon' viewBox='0 0 24 24'><path d='M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z'/></svg>";
  html += "</div>";
  html += "<div class='accordion-content' id='guide-content'>";
  html += "<h3>Blockchain-Like LoRa Network</h3>";
  html += "<p><strong>RadioDoge creates a decentralized mesh network that works like a blockchain for Dogecoin transactions!</strong></p>";
  html += "<p><strong>RECOMMENDED:</strong> Use the <strong>BROADCAST</strong> feature for Dogecoin transactions - it automatically finds devices with internet connectivity and forwards your transaction to the Dogecoin network!</p>";
  html += "<p><strong>How it works:</strong> Broadcast -> Mesh Propagation -> Gateway Discovery -> Blockchain Integration -> Response Relay</p>";
  
  html += "<h3>Web Interface</h3>";
  html += "<p><strong>Access:</strong> Connect to WiFi 'RadioDoge' (password: radiodoge) and open <code>http://192.168.4.1</code></p>";
  html += "<p><strong>Features:</strong> Click any section header to expand and access controls for ping, messages, transactions, broadcasts, and device configuration.</p>";
  
  html += "<h3>Mobile Access</h3>";
  html += "<p><strong>WiFi Access:</strong> Connect to 'RadioDoge' WiFi network (password: radiodoge) and open <code>http://192.168.4.1</code></p>";
  html += "<p><strong>Note:</strong> Use WiFi web interface for mobile access.</p>";
  
  html += "<h3>RECOMMENDED: Broadcast API Commands</h3>";
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;overflow-x:auto;word-wrap:break-word;white-space:pre-wrap;'>";
  html += "<p><strong>BROADCAST Dogecoin Transaction (Blockchain-Like):</strong><br><code>POST /api/broadcast</code><br>Body: <code style='word-break:break-all;'>type=transaction&priority=normal&message=0100000001...</code><br><em>No internet required! Automatically finds gateway and forwards to Dogecoin network</em></p>";
  html += "<p><strong>Send Direct Message:</strong><br><code>POST /api/message</code><br>Body: <code style='word-break:break-all;'>address=10.1.2&type=text&message=Hello!</code></p>";
  html += "<p><strong>Send Direct Transaction (Requires Internet):</strong><br><code>POST /api/transaction</code><br>Body: <code style='word-break:break-all;'>address=10.1.2&type=signed&data=0100000001...</code><br><em>Large transactions (>255 bytes) automatically use multipart packets</em></p>";
  html += "<p><strong>Broadcast General Message:</strong><br><code>POST /api/broadcast</code><br>Body: <code style='word-break:break-all;'>type=announcement&priority=normal&message=Network update</code><br><em>Large broadcasts (>255 bytes) automatically use multipart packets</em></p>";
  html += "<p><strong>System Status:</strong><br><code>GET /api/status</code></p>";
  html += "<p><strong>Multipart Status:</strong><br><code>GET /api/multipart/status</code> - View active multipart sessions</p>";
  html += "<p><strong>Set Address:</strong><br><code>POST /api/address</code><br>Body: <code style='word-break:break-all;'>region=10&community=1&node=5</code></p>";
  html += "</div>";
  
  html += "<h3>REST API</h3>";
  html += "<p><strong>Base URL:</strong> <code>http://192.168.4.1/api/</code></p>";
  html += "<p><strong>Format:</strong> JSON requests and responses</p>";
  html += "<p><strong>Methods:</strong> GET and POST supported</p>";
  
  html += "<h4>API Endpoints:</h4>";
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;'>";
  html += "<p><strong>Send Ping:</strong><br><code>GET /api/ping?region=10&community=1&node=2</code></p>";
  html += "<p><strong>Send Message:</strong><br><code>POST /api/message</code><br>Body: <code>address=10.1.2&type=text&text=Hello</code></p>";
  html += "<p><strong>Send Transaction (LoRa):</strong><br><code>POST /api/transaction</code><br>Body: <code>address=10.1.2&type=signed&data=0100000001...</code></p>";
  html += "<p><strong>Send Transaction (Internet):</strong><br><code>POST /api/transaction/send</code><br>Body: <code>transaction=0100000001...</code></p>";
  html += "<p><strong>Broadcast:</strong><br><code>POST /api/broadcast</code><br>Body: <code>type=announcement&priority=normal&message=Update</code></p>";
  html += "<p><strong>Get Status:</strong><br><code>GET /api/status</code></p>";
  html += "<p><strong>Set Address:</strong><br><code>POST /api/address</code><br>Body: <code>region=10&community=1&node=5</code></p>";
  html += "<p><strong>Gateway Config:</strong><br><code>GET /api/gateway/config</code> - Get stored gateway settings</p>";
  html += "<p><strong>Gateway Config Set:</strong><br><code>POST /api/gateway/config</code><br>Body: <code>type=core&ip=192.168.1.100&port=22555&username=user&password=pass</code></p>";
  html += "<p><strong>JSON-RPC:</strong><br><code>POST /api/jsonrpc</code><br>Body: <code>transaction=0100000001...&url=http://192.168.1.100:22555&rpcuser=user&rpcpass=pass</code><br>Returns: <code>{\"success\":true,\"transaction_id\":\"abc123...\"}</code> or <code>{\"success\":false,\"error\":\"error message\"}</code></p>";
  html += "<p><strong>Real-Time Logs:</strong><br><code>GET /api/logs</code> - Get system logs<br><code>GET /api/logs/text</code> - Get logs as plain text<br><code>POST /api/logs/send</code> - Send logs to other devices via LoRa</p>";
  html += "<p><strong>Queue Status:</strong><br><code>GET /api/queue/status</code> - View request queue status and pending requests</p>";
  html += "<p><strong>Multipart Status:</strong><br><code>GET /api/multipart/status</code> - View active multipart sessions</p>";
  html += "<p><strong>WiFi Status:</strong><br><code>GET /api/wifi</code> - Get WiFi connection status</p>";
  html += "<p><strong>Password Management:</strong><br><code>POST /api/password/change</code> - Change AP password<br><code>POST /api/password/reset</code> - Reset to default password<br><code>GET /api/password/status</code> - Get password status</p>";
  html += "<p><strong>Gateway Management:</strong><br><code>GET /api/gateway/status</code> - Get gateway status<br><code>POST /api/gateway/save</code> - Save gateway credentials<br><code>POST /api/gateway/clear</code> - Clear gateway credentials<br><code>POST /api/gateway/test</code> - Test gateway connection<br><code>GET /api/gateway/debug</code> - Gateway debug info<br><code>GET /api/gateway/load</code> - Load gateway config</p>";
  html += "<p><strong>RPC Support:</strong><br><code>POST /api/rpc</code> - Send RPC request</p>";
  html += "<p><strong>Internet Bridge:</strong><br><code>POST /api/bridge/enable</code> - Enable internet bridge<br><code>POST /api/bridge/disable</code> - Disable internet bridge<br><code>GET /api/bridge/status</code> - Get bridge status</p>";
  html += "</div>";
  
  html += "<h3>New Features</h3>";
  html += "<p><strong>Multipart Packets:</strong> Automatically split large transactions (>255 bytes) into multiple LoRa packets for transmission. Supports up to 4KB transactions across 20 packets with automatic reassembly.</p>";
  html += "<p><strong>Real-Time Logs:</strong> Monitor all system activity including display messages, network activity, and transaction processing in real-time.</p>";
  html += "<p><strong>Internet Gateway Support:</strong> Send transactions directly to Dogecoin Core, DogeBox, Dogecoin Wallet, or custom gateways via internet connection.</p>";
  html += "<p><strong>Persistent Gateway Configuration:</strong> Save gateway credentials that persist across device reboots.</p>";
  html += "<p><strong>JSON-RPC Support:</strong> Direct communication with Dogecoin Core nodes using JSON-RPC protocol.</p>";
  html += "<p><strong>Enhanced API:</strong> Complete REST API for programmatic control of all device functions.</p>";
  html += "<p><strong>Request Queuing System:</strong> Intelligent request queuing ensures reliable communication by processing requests sequentially and waiting for confirmations. Supports up to 10 pending requests with automatic timeout handling.</p>";
  html += "<p><strong>Internet Bridge:</strong> Share your internet connection with devices connected to the RadioDoge WiFi network. Enables transparent internet access for all connected devices.</p>";
  
  html += "<h3>Sending Dogecoin Transactions (Blockchain-Like)</h3>";
  html += "<p><strong>RECOMMENDED METHOD:</strong> Use <strong>BROADCAST</strong> for the best experience!</p>";
  html += "<p><strong>What you need:</strong> A <strong>signed Dogecoin transaction</strong> from your wallet</p>";
  html += "<p><strong>Why Use Broadcast?</strong></p>";
  html += "<ul style='margin:10px 0;padding-left:20px;'>";
  html += "<li><strong>No Internet Required:</strong> Your device doesn't need internet connectivity</li>";
  html += "<li><strong>Automatic Propagation:</strong> Transaction spreads through the entire RadioDoge network</li>";
  html += "<li><strong>Gateway Discovery:</strong> Automatically finds devices with internet connectivity</li>";
  html += "<li><strong>Full Response:</strong> Get detailed blockchain responses back to your device</li>";
  html += "<li><strong>Decentralized:</strong> No single point of failure - works like a blockchain</li>";
  html += "</ul>";
  html += "<p><strong>Transaction Types:</strong></p>";
  html += "<ul style='margin:10px 0;padding-left:20px;'>";
  html += "<li><strong>signed (Recommended):</strong> Complete signed transaction ready for broadcast - <em>This is what you should use</em></li>";
  html += "<li><strong>raw:</strong> Raw transaction data (advanced users)</li>";
  html += "<li><strong>utxo:</strong> UTXO data for transaction construction (advanced users)</li>";
  html += "</ul>";
  
  html += "<h4>Step-by-Step Guide (Broadcast Method)</h4>";
  html += "<ol style='margin:10px 0;padding-left:20px;'>";
  html += "<li><strong>Create Transaction:</strong> Use your Dogecoin wallet to create a transaction</li>";
  html += "<li><strong>Sign Transaction:</strong> Sign the transaction with your private key (this creates a <strong>signed transaction</strong>)</li>";
  html += "<li><strong>Export Signed Transaction:</strong> Get the signed transaction as a hex string from your wallet</li>";
  html += "<li><strong>Use BROADCAST:</strong> Go to <strong>BROADCAST MESSAGE</strong> section, select <strong>Transaction</strong> type, paste your signed transaction</li>";
  html += "<li><strong>Automatic Propagation:</strong> RadioDoge broadcasts to all devices in range, which rebroadcast to extend the network</li>";
  html += "<li><strong>Gateway Discovery:</strong> When a device with internet receives your transaction, it forwards to the Dogecoin network</li>";
  html += "<li><strong>Response Relay:</strong> The detailed blockchain response travels back through the mesh to your device</li>";
  html += "</ol>";
  
  html += "<h4>Method 1: LoRa to LoRa (RadioDoge Network)</h4>";
  html += "<p>Send transaction via LoRa to another RadioDoge device. If the receiving device has an internet gateway configured, it will automatically broadcast to the Dogecoin network and return the transaction ID.</p>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;overflow-x:auto;'>";
  html += "<p><strong>Web Interface:</strong></p>";
  html += "<p>1. Go to <strong>LoRa Messaging</strong> section</p>";
  html += "<p>2. Select <strong>Transaction</strong> tab</p>";
  html += "<p>3. Enter target address (e.g., 10.1.2)</p>";
  html += "<p>4. Paste your signed transaction hex</p>";
  html += "<p>5. Click <strong>SEND TRANSACTION</strong></p>";
  html += "</div>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;overflow-x:auto;'>";
  html += "<p><strong>API Example (cURL):</strong></p>";
  html += "<p><code>curl -X POST \"http://192.168.4.1/api/transaction\" \\</code></p>";
  html += "<p><code>&nbsp;&nbsp;-d \"address=10.1.2&type=signed&data=0100000001...\"</code></p>";
  html += "<p><strong>Response (Success):</strong></p>";
  html += "<p><code>{\"success\":true,\"action\":\"transaction\",\"target\":\"10.1.2\",\"message\":\"Transaction sent to 10.1.2\",\"internet_forwarded\":true,\"internet_response\":\"{\\\"success\\\":true,\\\"transaction_id\\\":\\\"abc123...\\\"}\"}</code></p>";
  html += "<p><strong>Response (Error):</strong></p>";
  html += "<p><code>{\"success\":true,\"action\":\"transaction\",\"target\":\"10.1.2\",\"message\":\"Transaction sent to 10.1.2\",\"internet_forwarded\":true,\"internet_response\":\"{\\\"success\\\":false,\\\"error\\\":\\\"transaction already in block chain\\\"}\"}</code></p>";
  html += "</div>";
  
  html += "<h4>Method 2: Direct to Internet Gateway</h4>";
  html += "<p>Send transaction directly to your configured internet gateway (Dogecoin Core, DogeBox, etc.) for immediate broadcast to the Dogecoin network.</p>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;overflow-x:auto;'>";
  html += "<p><strong>Web Interface:</strong></p>";
  html += "<p>1. Go to <strong>DOGE Internet Gateway</strong> section</p>";
  html += "<p>2. Configure your gateway (IP, Port, RPC credentials for Core)</p>";
  html += "<p>3. Click <strong>SAVE CREDENTIALS</strong></p>";
  html += "<p>4. Paste your signed transaction hex</p>";
  html += "<p>5. Click <strong>SEND TO INTERNET</strong></p>";
  html += "</div>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;overflow-x:auto;'>";
  html += "<p><strong>API Example - Direct Gateway (cURL):</strong></p>";
  html += "<p><code>curl -X POST \"http://192.168.4.1/api/transaction/send\" \\</code></p>";
  html += "<p><code>&nbsp;&nbsp;-d \"transaction=0100000001...\"</code></p>";
  html += "<p><strong>Response (Success):</strong></p>";
  html += "<p><code>{\"success\":true,\"action\":\"transaction_send\",\"message\":\"Transaction sent to stored gateway\",\"gateway_type\":\"core\",\"server_response\":\"{\\\"success\\\":true,\\\"transaction_id\\\":\\\"abc123...\\\"}\"}</code></p>";
  html += "<p><strong>Response (Error):</strong></p>";
  html += "<p><code>{\"success\":true,\"action\":\"transaction_send\",\"message\":\"Transaction sent to stored gateway\",\"gateway_type\":\"core\",\"server_response\":\"{\\\"success\\\":false,\\\"error\\\":\\\"insufficient funds\\\"}\"}</code></p>";
  html += "</div>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;overflow-x:auto;'>";
  html += "<p><strong>API Example - JSON-RPC (cURL):</strong></p>";
  html += "<p><code>curl -X POST \"http://192.168.4.1/api/jsonrpc\" \\</code></p>";
  html += "<p><code>&nbsp;&nbsp;-d \"transaction=0100000001...&url=http://192.168.1.100:22555&rpcuser=myuser&rpcpass=mypass\"</code></p>";
  html += "<p><strong>Response (Success):</strong></p>";
  html += "<p><code>{\"success\":true,\"action\":\"jsonrpc_send\",\"message\":\"JSON-RPC call sent to Dogecoin Core\",\"gateway\":\"http://192.168.1.100:22555\",\"parsed_response\":\"{\\\"success\\\":true,\\\"transaction_id\\\":\\\"abc123...\\\"}\"}</code></p>";
  html += "<p><strong>Response (Error):</strong></p>";
  html += "<p><code>{\"success\":true,\"action\":\"jsonrpc_send\",\"message\":\"JSON-RPC call sent to Dogecoin Core\",\"gateway\":\"http://192.168.1.100:22555\",\"parsed_response\":\"{\\\"success\\\":false,\\\"error\\\":\\\"transaction already in block chain\\\"}\"}</code></p>";
  html += "</div>";
  
  html += "<h4>Gateway Configuration</h4>";
  html += "<p><strong>Dogecoin Core:</strong> Most reliable, requires RPC credentials</p>";
  html += "<p><strong>DogeBox:</strong> Alternative gateway, requires endpoint configuration</p>";
  html += "<p><strong>Custom Gateway:</strong> Your own Dogecoin service</p>";
  html += "<p><strong>Note:</strong> Configure gateway in <strong>DOGE Internet Gateway</strong> section for automatic forwarding</p>";
  
  html += "<h4>Real-World Examples</h4>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;'>";
  html += "<h5 style='color:#ffc107;margin:0 0 10px 0;'>Example 1: Remote Village Payment</h5>";
  html += "<p><strong>Scenario:</strong> Send 100 DOGE from village A to village B (no internet in village A)</p>";
  html += "<p><strong>Steps:</strong></p>";
  html += "<ol style='margin:5px 0;padding-left:20px;'>";
  html += "<li>Create signed transaction in village A (with internet)</li>";
  html += "<li>Send via RadioDoge to village B device (address 10.1.2)</li>";
  html += "<li>Village B device has internet gateway configured</li>";
  html += "<li>Transaction automatically broadcasts to Dogecoin network</li>";
  html += "<li>Both devices receive transaction ID confirmation</li>";
  html += "</ol>";
  html += "<p><strong>API Call:</strong></p>";
  html += "<p><code>curl -X POST \"http://192.168.4.1/api/transaction\" -d \"address=10.1.2&type=signed&data=0100000001...\"</code></p>";
  html += "</div>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;'>";
  html += "<h5 style='color:#ffc107;margin:0 0 10px 0;'>Example 2: Emergency Internet Broadcast</h5>";
  html += "<p><strong>Scenario:</strong> You have internet but want to use RadioDoge's gateway for reliability</p>";
  html += "<p><strong>Steps:</strong></p>";
  html += "<ol style='margin:5px 0;padding-left:20px;'>";
  html += "<li>Configure Dogecoin Core gateway (192.168.1.100:22555)</li>";
  html += "<li>Save RPC credentials (username/password)</li>";
  html += "<li>Send transaction directly to gateway</li>";
  html += "<li>Get immediate transaction ID or error message</li>";
  html += "</ol>";
  html += "<p><strong>API Call:</strong></p>";
  html += "<p><code>curl -X POST \"http://192.168.4.1/api/transaction/send\" -d \"transaction=0100000001...\"</code></p>";
  html += "</div>";
  
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;'>";
  html += "<h5 style='color:#ffc107;margin:0 0 10px 0;'>Example 3: Mobile App Integration</h5>";
  html += "<p><strong>Scenario:</strong> Integrate RadioDoge into your mobile app</p>";
  html += "<p><strong>JavaScript Example:</strong></p>";
  html += "<pre style='margin:5px 0;overflow-x:auto;'><code>async function sendDogecoinTransaction(signedTx, targetAddress) {";
  html += "  const response = await fetch('http://192.168.4.1/api/transaction', {";
  html += "    method: 'POST',";
  html += "    headers: {'Content-Type': 'application/x-www-form-urlencoded'},";
  html += "    body: 'address=' + targetAddress + '&type=signed&data=' + signedTx";
  html += "  });";
  html += "  ";
  html += "  const result = await response.json();";
  html += "  ";
  html += "  if (result.internet_forwarded) {";
  html += "    const internetResult = JSON.parse(result.internet_response);";
  html += "    if (internetResult.success) {";
  html += "      console.log('Transaction ID:', internetResult.transaction_id);";
  html += "    } else {";
  html += "      console.error('Error:', internetResult.error);";
  html += "    }";
  html += "  }";
  html += "  ";
  html += "  return result;";
  html += "}</code></pre>";
  html += "</div>";
  
  html += "<h4>Important Notes</h4>";
  html += "<ul style='margin:10px 0;padding-left:20px;'>";
  html += "<li><strong>Always use signed transactions</strong> - Never send raw private keys</li>";
  html += "<li><strong>Test with small amounts first</strong> - Verify everything works</li>";
  html += "<li><strong>Check transaction ID</strong> - Verify on Dogecoin explorer</li>";
  html += "<li><strong>Keep RPC credentials secure</strong> - Don't share them</li>";
  html += "<li><strong>Monitor logs</strong> - Check Real-Time Logs for debugging</li>";
  html += "</ul>";
  
  html += "<h3>LoRa Network</h3>";
  html += "<p><strong>Address Format:</strong> Region.Community.Node (e.g., 10.1.3)</p>";
  html += "<p><strong>Range:</strong> Up to 10km in open areas</p>";
  html += "<p><strong>Frequency:</strong> 915 MHz (region dependent)</p>";
  html += "<p><strong>Power:</strong> Low power, battery friendly</p>";
  
  html += "<h3>Device Configuration</h3>";
  html += "<p><strong>Set Address:</strong> Use Device Configuration section to set your RadioDoge address</p>";
  html += "<p><strong>Address Range:</strong> Each component (Region, Community, Node) can be 0-255</p>";
  html += "<p><strong>Examples:</strong> 10.1.1, 10.1.2, 10.2.1, 255.255.255</p>";
  
  html += "<h3>Mobile Integration Examples</h3>";
  html += "<h4>Android (using curl):</h4>";
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;font-size:0.9em;'>";
  html += "<p># Send a message<br>curl -X POST \"http://192.168.4.1/api/message\" \\<br>&nbsp;&nbsp;-d \"address=10.1.2&type=text&text=Hello from Android!\"</p>";
  html += "<p># Send Dogecoin transaction<br>curl -X POST \"http://192.168.4.1/api/transaction\" \\<br>&nbsp;&nbsp;-d \"address=10.1.2&type=signed&data=0100000001...\"</p>";
  html += "</div>";
  
  html += "<h4>JavaScript (using fetch):</h4>";
  html += "<div style='background:#1a1a1a;padding:15px;border-radius:8px;margin:10px 0;font-family:monospace;font-size:0.9em;'>";
  html += "<p>// Send broadcast message<br>fetch('http://192.168.4.1/api/broadcast', {<br>&nbsp;&nbsp;method: 'POST',<br>&nbsp;&nbsp;headers: {<br>&nbsp;&nbsp;&nbsp;&nbsp;'Content-Type': 'application/x-www-form-urlencoded',<br>&nbsp;&nbsp;},<br>&nbsp;&nbsp;body: 'type=announcement&priority=normal&message=Hello from JavaScript!'<br>})<br>.then(response => response.json())<br>.then(data => console.log(data));</p>";
  html += "<br><p>// Send Dogecoin transaction<br>fetch('http://192.168.4.1/api/transaction', {<br>&nbsp;&nbsp;method: 'POST',<br>&nbsp;&nbsp;headers: {<br>&nbsp;&nbsp;&nbsp;&nbsp;'Content-Type': 'application/x-www-form-urlencoded',<br>&nbsp;&nbsp;},<br>&nbsp;&nbsp;body: 'address=10.1.2&type=signed&data=0100000001...'<br>})<br>.then(response => response.json())<br>.then(data => console.log(data));</p>";
  html += "</div>";
  
  html += "<h3>Important Notes</h3>";
  html += "<ul style='margin:10px 0;padding-left:20px;'>";
  html += "<li><strong>No Internet Required:</strong> RadioDoge works completely offline</li>";
  html += "<li><strong>P2P Network:</strong> Direct device-to-device communication</li>";
  html += "<li><strong>Range Limited:</strong> Devices must be within LoRa range</li>";
  html += "<li><strong>Transaction Security:</strong> Always verify transaction details before sending</li>";
  html += "<li><strong>Network Relay:</strong> Other RadioDoge devices can relay your transactions</li>";
  html += "<li><strong>Battery Life:</strong> LoRa is very power efficient</li>";
  html += "</ul>";
  
  html += "<h3>Troubleshooting</h3>";
  html += "<p><strong>Can't connect to WiFi:</strong> Make sure you're connecting to 'RadioDoge' network</p>";
  html += "<p><strong>API errors:</strong> Check that you're using the correct endpoint and parameters</p>";
  html += "<p><strong>No LoRa response:</strong> Verify target device is in range and powered on</p>";
  html += "<p><strong>Transaction failed:</strong> Ensure transaction is properly signed and formatted</p>";
  html += "</div>";
  html += "</div>";
  
  // Response Area
  html += "<div id='response' class='response' style='display:none;'></div>";
  
  // JavaScript
  html += "<script>";
  html += "function toggleAccordion(section){var content=document.getElementById(section+'-content');var icon=document.getElementById(section+'-icon');if(!content||!icon)return;var header=icon.parentElement;if(content.classList.contains('active')){content.classList.remove('active');header.classList.remove('active');icon.classList.remove('rotated');}else{content.classList.add('active');header.classList.add('active');icon.classList.add('rotated');}}";
  html += "function showResponse(msg){document.getElementById('response').style.display='block';document.getElementById('response').innerHTML=msg;}";
  html += "function sendPing(){var r=document.getElementById('pingRegion').value;var c=document.getElementById('pingCommunity').value;var n=document.getElementById('pingNode').value;fetch('/ping?region='+r+'&community='+c+'&node='+n).then(r=>r.text()).then(d=>showResponse('PING: '+d));}";
  html += "function pingAll(){fetch('/ping?broadcast=1').then(r=>r.text()).then(d=>showResponse('BROADCAST PING: '+d));}";
  html += "function sendMessage(){var addr=document.getElementById('msgAddress').value;var type=document.getElementById('msgType').value;var msg=document.getElementById('message').value;fetch('/message?address='+encodeURIComponent(addr)+'&type='+type+'&text='+encodeURIComponent(msg)).then(r=>r.text()).then(d=>showResponse('MESSAGE: '+d));}";
  html += "function sendTransaction(){var addr=document.getElementById('txAddress').value;var type=document.getElementById('txType').value;var tx=document.getElementById('transaction').value;fetch('/transaction?address='+encodeURIComponent(addr)+'&type='+type+'&data='+encodeURIComponent(tx)).then(r=>r.text()).then(d=>showResponse('TRANSACTION: '+d));}";
  html += "function sendBroadcast(){var type=document.getElementById('broadcastType').value;var priority=document.getElementById('broadcastPriority').value;var msg=document.getElementById('broadcastMessage').value;fetch('/broadcast?type='+type+'&priority='+priority+'&message='+encodeURIComponent(msg)).then(r=>r.text()).then(d=>showResponse('BROADCAST: '+d));}";
  html += "function sendACK(){var addr=document.getElementById('ackAddress').value;var type=document.getElementById('ackType').value;fetch('/ack?address='+encodeURIComponent(addr)+'&type='+type).then(r=>r.text()).then(d=>showResponse('ACK: '+d));}";
  html += "function setAddress(){var r=document.getElementById('region').value;var c=document.getElementById('community').value;var n=document.getElementById('node').value;fetch('/address?region='+r+'&community='+c+'&node='+n).then(r=>r.text()).then(d=>{showResponse('ADDRESS: '+d);updateStatusDisplay(r+'.'+c+'.'+n);});}";
  html += "function updateStatusDisplay(newAddress){var addressSpan=document.querySelector('.address-display');if(addressSpan){addressSpan.textContent=newAddress;}}";
  html += "function getStatus(){fetch('/status').then(r=>r.text()).then(d=>showResponse('STATUS: '+d));}";
  html += "function getMultipartStatus(){fetch('/api/multipart/status').then(r=>r.json()).then(d=>{document.getElementById('response').style.display='block';document.getElementById('response').innerHTML='MULTIPART STATUS:<br><pre>'+JSON.stringify(d,null,2)+'</pre>';});}";
  html += "function getDetailedLogs(){fetch('/api/logs/text').then(r=>r.text()).then(d=>{document.getElementById('response').style.display='block';document.getElementById('response').innerHTML='<h3>DETAILED SYSTEM LOGS</h3><pre style=\"max-height:400px;overflow-y:auto;background:#000;color:#0f0;padding:10px;\">'+d+'</pre>';}).catch(e=>{document.getElementById('response').style.display='block';document.getElementById('response').innerHTML='<h3>ERROR LOADING LOGS</h3><p>Error: '+e.message+'</p>';});}";
  html += "function getCurrentAddress(){fetch('/api/status').then(r=>r.json()).then(d=>{document.getElementById('response').style.display='block';document.getElementById('response').innerHTML='<h3>CURRENT ADDRESS</h3><pre>'+JSON.stringify(d,null,2)+'</pre>';});}";
  html += "function clearLoRaConfig(){if(confirm('Are you sure you want to clear stored LoRa configuration? This will reset to default values.')){fetch('/api/lora/clear',{method:'POST'}).then(r=>r.json()).then(d=>{showResponse('LORA CONFIG: '+JSON.stringify(d,null,2));setTimeout(()=>location.reload(),2000);});}}";
  html += "function changePassword(){var newPass=document.getElementById('newPassword').value;var confirmPass=document.getElementById('confirmPassword').value;if(!newPass||!confirmPass){showResponse('Please enter both new password and confirmation');return;}if(newPass!==confirmPass){showResponse('Passwords do not match');return;}if(newPass.length<8||newPass.length>32){showResponse('Password must be 8-32 characters long');return;}var hasLetter=/[a-zA-Z]/.test(newPass);var hasNumber=/[0-9]/.test(newPass);if(!hasLetter||!hasNumber){showResponse('Password must contain at least one letter and one number');return;}fetch('/api/password/change',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'password='+encodeURIComponent(newPass)}).then(r=>r.json()).then(d=>{document.getElementById('passwordStatus').style.display='block';document.getElementById('passwordStatus').innerHTML=JSON.stringify(d,null,2);if(d.success){setTimeout(()=>{alert('Password changed successfully! You will be disconnected. Please reconnect with the new password.');location.reload();},2000);}});}";
  html += "function resetPassword(){if(confirm('Are you sure you want to reset the password to default (radiodoge)? This will disconnect all current users.')){fetch('/api/password/reset',{method:'POST'}).then(r=>r.json()).then(d=>{document.getElementById('passwordStatus').style.display='block';document.getElementById('passwordStatus').innerHTML=JSON.stringify(d,null,2);setTimeout(()=>{alert('Password reset to default! You will be disconnected. Please reconnect with password: radiodoge');location.reload();},2000);});}}";
  html += "function getPasswordStatus(){fetch('/api/password/status').then(r=>r.json()).then(d=>{document.getElementById('passwordStatus').style.display='block';document.getElementById('passwordStatus').innerHTML=JSON.stringify(d,null,2);});}";
  html += "function connectWiFi(){var ssid=document.getElementById('wifiSSID').value;var password=document.getElementById('wifiPassword').value;if(!ssid){showResponse('Please enter WiFi SSID');return;}fetch('/api/wifi/connect',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'ssid='+encodeURIComponent(ssid)+'&password='+encodeURIComponent(password)}).then(r=>r.json()).then(d=>{document.getElementById('wifiStatus').style.display='block';document.getElementById('wifiStatus').innerHTML=JSON.stringify(d,null,2);if(d.success){setTimeout(()=>location.reload(),2000);}});}";
  html += "function disconnectWiFi(){fetch('/api/wifi/disconnect',{method:'POST'}).then(r=>r.json()).then(d=>{document.getElementById('wifiStatus').style.display='block';document.getElementById('wifiStatus').innerHTML=JSON.stringify(d,null,2);setTimeout(()=>location.reload(),2000);});}";
  html += "function getWiFiStatus(){fetch('/api/wifi').then(r=>r.json()).then(d=>{document.getElementById('wifiStatus').style.display='block';document.getElementById('wifiStatus').innerHTML=JSON.stringify(d,null,2);});}";
  html += "function clearWiFiCredentials(){if(confirm('Are you sure you want to clear stored WiFi credentials? This will prevent automatic reconnection on boot.')){fetch('/api/wifi/clear',{method:'POST'}).then(r=>r.json()).then(d=>{document.getElementById('wifiStatus').style.display='block';document.getElementById('wifiStatus').innerHTML=JSON.stringify(d,null,2);setTimeout(()=>location.reload(),2000);});}}";
  html += "function enableBridge(){fetch('/api/bridge/enable',{method:'POST'}).then(r=>r.json()).then(d=>{document.getElementById('bridgeStatus').style.display='block';document.getElementById('bridgeStatus').innerHTML=JSON.stringify(d,null,2);if(d.success){setTimeout(()=>location.reload(),2000);}});}";
  html += "function disableBridge(){fetch('/api/bridge/disable',{method:'POST'}).then(r=>r.json()).then(d=>{document.getElementById('bridgeStatus').style.display='block';document.getElementById('bridgeStatus').innerHTML=JSON.stringify(d,null,2);setTimeout(()=>location.reload(),2000);});}";
  html += "function getBridgeStatus(){fetch('/api/bridge/status').then(r=>r.json()).then(d=>{document.getElementById('bridgeStatus').style.display='block';document.getElementById('bridgeStatus').innerHTML=JSON.stringify(d,null,2);});}";
  html += "function testProxy(){var url=prompt('Enter website URL to test (e.g., google.com):','google.com');if(url){window.open('/proxy?url='+encodeURIComponent(url),'_blank');}}";
  html += "function openGoogle(){window.open('/proxy?url=google.com','_blank');}";
  html += "function updateGatewayFields(){var type=document.getElementById('gatewayType').value;var customFields=document.getElementById('customFields');var rpcFields=document.getElementById('rpcFields');var ipField=document.getElementById('ipField');var gatewayInfo=document.getElementById('gatewayInfo');var description=document.getElementById('gatewayDescription');var endpoint=document.getElementById('gatewayEndpoint');var requirements=document.getElementById('gatewayRequirements');var endpointField=document.getElementById('endpointField');if(type==='none'){ipField.style.display='none';customFields.style.display='none';rpcFields.style.display='none';gatewayInfo.style.display='none';}else if(type==='core'){ipField.style.display='block';customFields.style.display='block';rpcFields.style.display='block';gatewayInfo.style.display='block';endpointField.style.display='none';description.innerHTML='Connect to your local Dogecoin Core node using RPC for transaction broadcasting.';endpoint.innerHTML='Endpoint: http://[IP]:[PORT] (RPC)';requirements.innerHTML='Requirements: Enable RPC in dogecoin.conf (server=1, rpcuser, rpcpassword, rpcport)';document.getElementById('gatewayIp').placeholder='192.168.1.100';document.getElementById('gatewayPort').value='22555';document.getElementById('gatewayEndpoint').value='';}else if(type==='dogebox'){ipField.style.display='block';customFields.style.display='block';rpcFields.style.display='none';gatewayInfo.style.display='block';endpointField.style.display='block';description.innerHTML='Connect to DogeBox API for transaction broadcasting.';endpoint.innerHTML='Endpoint: http://[IP]:[PORT][ENDPOINT]';requirements.innerHTML='Requirements: DogeBox running on specified port';document.getElementById('gatewayIp').placeholder='192.168.1.100';document.getElementById('gatewayPort').value='420';document.getElementById('gatewayEndpoint').value='/dogebox-api/tx/send';}else if(type==='wallet'){ipField.style.display='block';customFields.style.display='block';rpcFields.style.display='none';gatewayInfo.style.display='block';endpointField.style.display='block';description.innerHTML='Connect to Dogecoin Wallet API for transaction broadcasting.';endpoint.innerHTML='Endpoint: http://[IP]:[PORT][ENDPOINT]';requirements.innerHTML='Requirements: Dogecoin Wallet with API enabled';document.getElementById('gatewayIp').placeholder='192.168.1.100';document.getElementById('gatewayPort').value='80';document.getElementById('gatewayEndpoint').value='/tx/send';}else if(type==='custom'){ipField.style.display='block';customFields.style.display='block';rpcFields.style.display='none';gatewayInfo.style.display='block';endpointField.style.display='block';description.innerHTML='Connect to a custom gateway endpoint.';endpoint.innerHTML='Endpoint: http://[IP]:[PORT][ENDPOINT]';requirements.innerHTML='Requirements: Custom gateway accepting POST requests with transaction data';document.getElementById('gatewayIp').placeholder='192.168.1.100';document.getElementById('gatewayPort').placeholder='8080';document.getElementById('gatewayEndpoint').placeholder='/api/push/tx';}}";
  html += "function sendToGateway(){var type=document.getElementById('gatewayType').value;if(type==='none'){showResponse('Please select a gateway type');return;}var ip=document.getElementById('gatewayIp').value;var tx=document.getElementById('gatewayTransaction').value;if(!tx){showResponse('Please enter transaction data');return;}if(!ip){showResponse('Please enter IP address');return;}var url='';var body='';var endpoint='';if(type==='core'){var rpcUser=document.getElementById('rpcUsername').value;var rpcPass=document.getElementById('rpcPassword').value;var port=document.getElementById('gatewayPort').value||'22555';if(!rpcUser||!rpcPass){showResponse('Please enter RPC username and password');return;}if(!port){showResponse('Please enter port for CORE gateway');return;}url='http://'+ip+':'+port;endpoint='/api/jsonrpc';body='transaction='+encodeURIComponent(tx)+'&url='+encodeURIComponent(url)+'&rpcuser='+encodeURIComponent(rpcUser)+'&rpcpass='+encodeURIComponent(rpcPass);}else if(type==='dogebox'){var port=document.getElementById('gatewayPort').value||'420';var endpoint=document.getElementById('gatewayEndpoint').value||'/dogebox-api/tx/send';if(!port||!endpoint){showResponse('Please enter port and endpoint for DogeBox gateway');return;}url='http://'+ip+':'+port+endpoint;endpoint='/api/gateway';body='transaction='+encodeURIComponent(tx);}else if(type==='wallet'){var port=document.getElementById('gatewayPort').value||'80';var endpoint=document.getElementById('gatewayEndpoint').value||'/tx/send';if(!port||!endpoint){showResponse('Please enter port and endpoint for Wallet gateway');return;}url='http://'+ip+':'+port+endpoint;endpoint='/api/gateway';body='transaction='+encodeURIComponent(tx);}else if(type==='custom'){var port=document.getElementById('gatewayPort').value;var endpoint=document.getElementById('gatewayEndpoint').value;if(!port||!endpoint){showResponse('Please enter port and endpoint for custom gateway');return;}url='http://'+ip+':'+port+endpoint;endpoint='/api/gateway';body='transaction='+encodeURIComponent(tx);}fetch(endpoint,{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body}).then(r=>r.json()).then(d=>{document.getElementById('gatewayStatus').style.display='block';document.getElementById('gatewayStatus').innerHTML=JSON.stringify(d,null,2);});}";
  html += "function testGateway(){var type=document.getElementById('gatewayType').value;if(type==='none'){showResponse('Please select a gateway type');return;}var ip=document.getElementById('gatewayIp').value;if(!ip){showResponse('Please enter IP address');return;}var url='';if(type==='core'){var rpcUser=document.getElementById('rpcUsername').value;var rpcPass=document.getElementById('rpcPassword').value;var port=document.getElementById('gatewayPort').value||'22555';if(!rpcUser||!rpcPass){showResponse('Please enter RPC username and password');return;}if(!port){showResponse('Please enter port for CORE gateway');return;}url='http://'+ip+':'+port;}else if(type==='dogebox'){var port=document.getElementById('gatewayPort').value||'420';var endpoint=document.getElementById('gatewayEndpoint').value||'/dogebox-api/tx/send';if(!port||!endpoint){showResponse('Please enter port and endpoint for DogeBox gateway');return;}url='http://'+ip+':'+port+endpoint;}else if(type==='wallet'){var port=document.getElementById('gatewayPort').value||'80';var endpoint=document.getElementById('gatewayEndpoint').value||'/tx/send';if(!port||!endpoint){showResponse('Please enter port and endpoint for Wallet gateway');return;}url='http://'+ip+':'+port+endpoint;}else if(type==='custom'){var port=document.getElementById('gatewayPort').value;var endpoint=document.getElementById('gatewayEndpoint').value;if(!port||!endpoint){showResponse('Please enter port and endpoint for custom gateway');return;}url='http://'+ip+':'+port+endpoint;}fetch('/api/gateway/test',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'url='+encodeURIComponent(url)}).then(r=>r.json()).then(d=>{document.getElementById('gatewayStatus').style.display='block';document.getElementById('gatewayStatus').innerHTML=JSON.stringify(d,null,2);});}";
  // The stored RPC password is never sent back by /api/gateway/load (it is write-only).
  // Leave the field blank and use its placeholder to show whether one is saved; an
  // empty field on save means "keep the stored password".
  html += "function loadStoredGatewayCredentials(){fetch('/api/gateway/load').then(r=>r.json()).then(d=>{if(d.success&&d.gateway){document.getElementById('gatewayType').value=d.gateway.type||'none';updateGatewayFields();document.getElementById('gatewayIp').value=d.gateway.ip||'';document.getElementById('gatewayPort').value=d.gateway.port||'';document.getElementById('gatewayEndpoint').value=d.gateway.endpoint||'';document.getElementById('rpcUsername').value=d.gateway.username||'';var p=document.getElementById('rpcPassword');p.value='';p.placeholder=d.gateway.has_password?'(saved - leave blank to keep)':'(none saved)';}}).catch(e=>{console.log('No stored gateway credentials found');});}";
  html += "function saveGatewayCredentials(){var type=document.getElementById('gatewayType').value;if(type==='none'){showResponse('Please select a gateway type');return;}var ip=document.getElementById('gatewayIp').value;var port=document.getElementById('gatewayPort').value;var endpoint=document.getElementById('gatewayEndpoint').value;var username=document.getElementById('rpcUsername').value;var password=document.getElementById('rpcPassword').value;if(!ip){showResponse('Please enter IP address');return;}if(!port){showResponse('Please enter port');return;}if(type!='core'&&!endpoint){showResponse('Please enter endpoint');return;}if(type==='core'&&(!username||!password)){showResponse('Please enter RPC username and password for CORE gateway');return;}var button=event.target;button.disabled=true;button.textContent='Saving...';fetch('/api/gateway/save',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'type='+encodeURIComponent(type)+'&ip='+encodeURIComponent(ip)+'&port='+encodeURIComponent(port)+'&endpoint='+encodeURIComponent(endpoint)+'&username='+encodeURIComponent(username)+'&password='+encodeURIComponent(password)}).then(r=>r.json()).then(d=>{document.getElementById('gatewayStatus').style.display='block';document.getElementById('gatewayStatus').innerHTML=JSON.stringify(d,null,2);if(d.success){button.textContent='Saved!';setTimeout(()=>{button.disabled=false;button.textContent='SAVE CREDENTIALS';},2000);}else{button.disabled=false;button.textContent='SAVE CREDENTIALS';}}).catch(e=>{document.getElementById('gatewayStatus').style.display='block';document.getElementById('gatewayStatus').innerHTML='Error: '+e.message;button.disabled=false;button.textContent='SAVE CREDENTIALS';});}";
  html += "function clearGatewayCredentials(){if(confirm('Are you sure you want to clear stored gateway credentials?')){fetch('/api/gateway/clear',{method:'POST'}).then(r=>r.json()).then(d=>{document.getElementById('gatewayStatus').style.display='block';document.getElementById('gatewayStatus').innerHTML=JSON.stringify(d,null,2);setTimeout(()=>location.reload(),2000);});}}";
  html += "var autoRefreshInterval=null;function refreshLogs(){fetch('/api/logs').then(r=>r.json()).then(d=>{if(d.success){var container=document.getElementById('logsContainer');container.innerHTML='';d.logs.forEach(log=>{var entry=document.createElement('div');entry.className='log-entry';if(log.includes('ERROR')||log.includes('Error')){entry.className+=' error';entry.style.backgroundColor='#f8d7da';entry.style.borderLeft='4px solid #dc3545';entry.style.color='#000000';}else if(log.includes('WARNING')||log.includes('Warning')){entry.className+=' warning';entry.style.backgroundColor='#fff3cd';entry.style.borderLeft='4px solid #ffc107';entry.style.color='#000000';}else if(log.includes('INFO')||log.includes('Info')){entry.className+=' info';entry.style.backgroundColor='#d1ecf1';entry.style.borderLeft='4px solid #17a2b8';entry.style.color='#000000';}else if(log.includes('DEBUG')||log.includes('Debug')){entry.className+=' debug';entry.style.backgroundColor='#e2e3e5';entry.style.borderLeft='4px solid #6c757d';entry.style.color='#000000';}else if(log.includes('DOGECOIN_RESPONSE')||log.includes('TX_CONFIRM')||log.includes('BC_CONFIRM')||log.includes('confirmation')){entry.className+=' success';entry.style.backgroundColor='#d4edda';entry.style.borderLeft='4px solid #28a745';entry.style.color='#000000';}entry.textContent=log;container.appendChild(entry);});container.scrollTop=container.scrollHeight;}}).catch(e=>{console.error('Error fetching logs:',e);});}function clearLogs(){if(confirm('Clear all logs? This will remove all log entries from memory.')){document.getElementById('logsContainer').innerHTML='<div class=\"log-entry\">Logs cleared</div>';}}function toggleAutoRefresh(){var btn=document.getElementById('autoRefreshBtn');if(autoRefreshInterval){clearInterval(autoRefreshInterval);autoRefreshInterval=null;btn.textContent='AUTO REFRESH: OFF';}else{autoRefreshInterval=setInterval(refreshLogs,2000);btn.textContent='AUTO REFRESH: ON';}}";
  html += "function sendLogs(){var address=document.getElementById('logAddress').value;var type=document.getElementById('logType').value;var statusDiv=document.getElementById('logSendStatus');if(!address){statusDiv.innerHTML='<div class=\"error\">Please enter a target address</div>';statusDiv.style.display='block';return;}fetch('/api/logs/send',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'address='+encodeURIComponent(address)+'&type='+encodeURIComponent(type)+'&logs='+encodeURIComponent(document.getElementById('logsContainer').innerText)}).then(r=>r.json()).then(d=>{statusDiv.innerHTML='<div class=\"success\">Logs sent successfully to '+address+'</div>';statusDiv.style.display='block';setTimeout(()=>{statusDiv.style.display='none';},3000);}).catch(e=>{statusDiv.innerHTML='<div class=\"error\">Error sending logs: '+e.message+'</div>';statusDiv.style.display='block';});}";
  html += "window.onload=function(){loadStoredGatewayCredentials();refreshLogs();toggleAutoRefresh();};";
  html += "</script></div></body></html>";
  
  server.send(200, "text/html", html);
}

void handlePing() {
  if (server.hasArg("broadcast") && server.arg("broadcast") == "1") {
    // Broadcast ping to all devices
    SetDestinationAsBroadcast();
    SendPing(dest);
    server.send(200, "text/plain", "Broadcast ping sent to all devices");
  } else {
    // Send ping to specific address
    int region = server.hasArg("region") ? server.arg("region").toInt() : 10;
    int community = server.hasArg("community") ? server.arg("community").toInt() : 1;
    int node = server.hasArg("node") ? server.arg("node").toInt() : 2;
    
    nodeAddress pingDest = {region, community, node};
    SendPing(pingDest);
    server.send(200, "text/plain", "Ping sent to " + String(region) + "." + String(community) + "." + String(node));
  }
}

void handleMessage() {
  if (server.hasArg("text")) {
    String message = server.arg("text");
    String address = server.hasArg("address") ? server.arg("address") : "10.1.2";
    String type = server.hasArg("type") ? server.arg("type") : "text";
    
    // Parse address
    int firstDot = address.indexOf('.');
    int secondDot = address.indexOf('.', firstDot + 1);
    int region = address.substring(0, firstDot).toInt();
    int community = address.substring(firstDot + 1, secondDot).toInt();
    int node = address.substring(secondDot + 1).toInt();
    
    nodeAddress msgDest = {region, community, node};
    dest = msgDest;
    
    // Convert message to bytes
    int messageLength = message.length();
    if (messageLength > 200) messageLength = 200;  // Limit message size
    
    // Prepare message packet
    uint8_t messagePacket[256];
    messagePacket[0] = (uint8_t)MESSAGE;
    messagePacket[1] = 6;  // Header size
    messagePacket[2] = local.region;
    messagePacket[3] = local.community;
    messagePacket[4] = local.node;
    messagePacket[5] = dest.region;
    messagePacket[6] = dest.community;
    messagePacket[7] = dest.node;
    
    // Add message content
    for (int i = 0; i < messageLength; i++) {
      messagePacket[8 + i] = message[i];
    }
    
    // Send the message
    SendLoRaAndWait(messagePacket, 8 + messageLength);
    DisplayTXMessage(message, dest);
    
    server.send(200, "text/plain", "Message sent to " + address + ": " + message);
  } else {
    server.send(400, "text/plain", "No message provided");
  }
}

// v0.4.2 — Validate an address the operator is asking this board to adopt.
//
// `String::toInt()` returns a long and the octets are uint8_t, so 999 silently
// became 231; and nothing rejected 255.255.255, which is the reserved broadcast
// address. A board that adopted it would match CheckIfPacketForMe() *and*
// CheckIfPacketIsGlobalBroadcast() for every packet on the air, handling each one
// twice, and could never be addressed individually again — recoverable only by
// clearing NVS.
//
// Only applies to the board's own address. A *destination* of 255.255.255 is
// legitimate: that is how a broadcast is addressed.
bool parseLocalAddressOctets(long region, long community, long node, nodeAddress &out) {
  if (region < 0 || region > 255 || community < 0 || community > 255 || node < 0 || node > 255) {
    return false;
  }
  if (region == 255 && community == 255 && node == 255) {
    return false;  // reserved broadcast address
  }
  out.region = (uint8_t)region;
  out.community = (uint8_t)community;
  out.node = (uint8_t)node;
  return true;
}

void handleAddress() {
  if (server.hasArg("region") && server.hasArg("community") && server.hasArg("node")) {
    // Set new address
    nodeAddress requested;
    if (!parseLocalAddressOctets(server.arg("region").toInt(),
                                 server.arg("community").toInt(),
                                 server.arg("node").toInt(), requested)) {
      server.send(400, "text/plain",
                  "Invalid address: each octet must be 0-255 and 255.255.255 is reserved for broadcast");
      return;
    }
    local = requested;
    InitControlMessages();
    DisplayLocalAddress(local);
    // Save LoRa configuration to NVS
    saveLoRaConfiguration(local.region, local.community, local.node);
    server.send(200, "text/plain", "Address set to " + String(local.region) + "." + String(local.community) + "." + String(local.node));
  } else {
    // Get current address
    server.send(200, "text/plain", String(local.region) + "." + String(local.community) + "." + String(local.node));
  }
}

void handleTransaction() {
  if (server.hasArg("data")) {
    String transaction = server.arg("data");
    String address = server.hasArg("address") ? server.arg("address") : "10.1.2";
    String type = server.hasArg("type") ? server.arg("type") : "signed";
    
    // Parse address
    int firstDot = address.indexOf('.');
    int secondDot = address.indexOf('.', firstDot + 1);
    int region = address.substring(0, firstDot).toInt();
    int community = address.substring(firstDot + 1, secondDot).toInt();
    int node = address.substring(secondDot + 1).toInt();
    
    nodeAddress txDest = {region, community, node};
    dest = txDest;
    
    // Convert transaction to bytes
    int txLength = transaction.length();
    if (txLength > 200) txLength = 200;  // Limit transaction size
    
    // Prepare transaction packet
    uint8_t txPacket[256];
    txPacket[0] = (uint8_t)HOST_FORMED_PACKET;  // Use custom packet type for transactions
    txPacket[1] = 6;  // Header size
    txPacket[2] = local.region;
    txPacket[3] = local.community;
    txPacket[4] = local.node;
    txPacket[5] = dest.region;
    txPacket[6] = dest.community;
    txPacket[7] = dest.node;
    
    // Add transaction content
    for (int i = 0; i < txLength; i++) {
      txPacket[8 + i] = transaction[i];
    }
    
    // Send the transaction
    SendLoRaAndWait(txPacket, 8 + txLength);
    DisplayTXMessage("Transaction: " + transaction.substring(0, 20) + "...", dest);
    
    // Forward to internet if connected
    if (internet_connected) {
      String internetResponse = sendTransactionToInternet(transaction);
      debugPrintln("Transaction also forwarded to internet gateway");
      debugPrintln("Internet response: " + internetResponse);
    }
    
    server.send(200, "text/plain", "Transaction sent to " + address + " (" + type + ")" + (internet_connected ? " + Internet" : ""));
  } else {
    server.send(400, "text/plain", "No transaction data provided");
  }
}

void handleBroadcast() {
  if (server.hasArg("message")) {
    String message = server.arg("message");
    String type = server.hasArg("type") ? server.arg("type") : "announcement";
    String priority = server.hasArg("priority") ? server.arg("priority") : "normal";
    
    // Set destination as broadcast
    SetDestinationAsBroadcast();
    
    // Convert message to bytes
    int messageLength = message.length();
    if (messageLength > 200) messageLength = 200;  // Limit message size
    
    // Prepare broadcast packet
    uint8_t broadcastPacket[256];
    broadcastPacket[0] = (uint8_t)MESSAGE;  // Use MESSAGE type for broadcast
    broadcastPacket[1] = 6;  // Header size
    broadcastPacket[2] = local.region;
    broadcastPacket[3] = local.community;
    broadcastPacket[4] = local.node;
    broadcastPacket[5] = 255;  // Broadcast address
    broadcastPacket[6] = 255;  // Broadcast address
    broadcastPacket[7] = 255;  // Broadcast address
    
    // Add message content
    for (int i = 0; i < messageLength; i++) {
      broadcastPacket[8 + i] = message[i];
    }
    
    // Send the broadcast
    SendLoRaAndWait(broadcastPacket, 8 + messageLength);
    DisplayBroadcastMessage("Broadcast: " + message, local);
    
    server.send(200, "text/plain", "Broadcast sent (" + type + ", " + priority + "): " + message);
  } else {
    server.send(400, "text/plain", "No broadcast message provided");
  }
}

void handleACK() {
  if (server.hasArg("address")) {
    String address = server.arg("address");
    String type = server.hasArg("type") ? server.arg("type") : "received";
    
    // Parse address
    int firstDot = address.indexOf('.');
    int secondDot = address.indexOf('.', firstDot + 1);
    int region = address.substring(0, firstDot).toInt();
    int community = address.substring(firstDot + 1, secondDot).toInt();
    int node = address.substring(secondDot + 1).toInt();
    
    nodeAddress ackDest = {region, community, node};
    SendACK(ackDest);
    
    server.send(200, "text/plain", "ACK sent to " + address + " (" + type + ")");
  } else {
    server.send(400, "text/plain", "No ACK target address provided");
  }
}

void handleStatus() {
  String status = "RadioDoge - Ready\n";
  status += "Address: " + String(local.region) + "." + String(local.community) + "." + String(local.node) + "\n";
  status += "LoRa: Active\n";
  status += "Display: Working\n";
  status += "WiFi: " + String(ap_ssid) + "\n";
  status += "Uptime: " + String(millis() / 1000) + " seconds\n";
  status += "Free Memory: " + String(ESP.getFreeHeap()) + " bytes";
  server.send(200, "text/plain", status);
}


// JSON API Handlers
void handleApiPing() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("broadcast") && server.arg("broadcast") == "1") {
    SetDestinationAsBroadcast();
    SendPing(dest);
    response += "\"action\":\"broadcast_ping\",";
    response += "\"message\":\"Broadcast ping sent to all devices\"";
  } else {
    int region = server.hasArg("region") ? server.arg("region").toInt() : 10;
    int community = server.hasArg("community") ? server.arg("community").toInt() : 1;
    int node = server.hasArg("node") ? server.arg("node").toInt() : 2;
    
    nodeAddress pingDest = {region, community, node};
    SendPing(pingDest);
    response += "\"action\":\"ping\",";
    response += "\"target\":\"" + String(region) + "." + String(community) + "." + String(node) + "\",";
    response += "\"message\":\"Ping sent to " + String(region) + "." + String(community) + "." + String(node) + "\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiMessage() {
  addLog("[API] Message endpoint called - IP: " + server.client().remoteIP().toString());
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("text")) {
    String message = server.arg("text");
    String address = server.hasArg("address") ? server.arg("address") : "10.1.2";
    String type = server.hasArg("type") ? server.arg("type") : "text";
    addLog("[API] Sending message via LoRa - To: " + address + ", Type: " + type + ", Length: " + String(message.length()));
    
    // Parse address
    int dots[2];
    int dotCount = 0;
    for (int i = 0; i < address.length() && dotCount < 2; i++) {
      if (address.charAt(i) == '.') {
        dots[dotCount] = i;
        dotCount++;
      }
    }
    
    if (dotCount == 2) {
      int region = address.substring(0, dots[0]).toInt();
      int community = address.substring(dots[0] + 1, dots[1]).toInt();
      int node = address.substring(dots[1] + 1).toInt();
      
      nodeAddress msgDest = {region, community, node};
      SendMessage(msgDest, message, type);
      
      response += "\"action\":\"message\",";
      response += "\"target\":\"" + address + "\",";
      response += "\"type\":\"" + type + "\",";
      response += "\"message\":\"Message sent to " + address + "\"";
    } else {
      response += "\"success\":false,";
      response += "\"error\":\"Invalid address format. Use REGION.COMMUNITY.NODE\"";
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"No message text provided\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiTransaction() {
  addLog("[API] Transaction endpoint called - IP: " + server.client().remoteIP().toString());
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("data")) {
    String transaction = server.arg("data");
    String address = server.hasArg("address") ? server.arg("address") : "10.1.2";
    String type = server.hasArg("type") ? server.arg("type") : "signed";
    addLog("[API] Sending transaction via LoRa - To: " + address + ", Type: " + type + ", Length: " + String(transaction.length()));
    
    // Parse address
    int dots[2];
    int dotCount = 0;
    for (int i = 0; i < address.length() && dotCount < 2; i++) {
      if (address.charAt(i) == '.') {
        dots[dotCount] = i;
        dotCount++;
      }
    }
    
    if (dotCount == 2) {
      int region = address.substring(0, dots[0]).toInt();
      int community = address.substring(dots[0] + 1, dots[1]).toInt();
      int node = address.substring(dots[1] + 1).toInt();
      
      nodeAddress txDest = {region, community, node};
      
      // Check if transaction is too large for single packet
      String txData = type + ":" + transaction;
      if (txData.length() > 255) {
        // Use multipart for large transactions
        SendMultipartTransaction(txDest, transaction, type);
        addLog("[API] Using multipart for large transaction - " + String(txData.length()) + " bytes");
      } else {
        // Use regular single packet for small transactions
        SendTransaction(txDest, transaction, type);
      }
      
      // Forward to gateway if configured
      String internetResponse = "";
      bool gateway_forwarded = false;
      
      if (gateway_type != "none" && gateway_ip.length() > 0) {
        String gatewayUrl = "http://" + gateway_ip + ":" + gateway_port;
        if (gateway_type != "core" && gateway_endpoint.length() > 0) {
          gatewayUrl += gateway_endpoint;
        }
        
        if (gateway_type == "core") {
          // Use JSON-RPC for Dogecoin Core
          String rawResponse = sendJsonRpcToGateway(transaction, gatewayUrl, gateway_username, gateway_password);
          internetResponse = parseDogecoinCoreResponse(rawResponse);
          addLog("Transaction forwarded to stored Dogecoin Core gateway");
          gateway_forwarded = true;
        } else {
          // Use regular HTTP POST for other gateways
          internetResponse = sendTransactionToCustomGateway(transaction, gatewayUrl);
          addLog("Transaction forwarded to stored " + gateway_type + " gateway");
          gateway_forwarded = true;
        }
      } else if (internet_connected) {
        // Fallback to default internet gateway only if no local gateway and internet is available
        internetResponse = sendTransactionToInternet(transaction);
        addLog("Transaction forwarded to default internet gateway");
        gateway_forwarded = true;
      }
      
      // Check if multipart was used
      if (txData.length() > 255) {
        response += "\"action\":\"multipart_transaction\",";
        response += "\"target\":\"" + address + "\",";
        response += "\"type\":\"" + type + "\",";
        response += "\"message\":\"Large transaction sent via multipart to " + address + "\",";
        response += "\"parts\":\"" + String((txData.length() + MULTIPART_CHUNK_SIZE - 1) / MULTIPART_CHUNK_SIZE) + "\"";
      } else {
        response += "\"action\":\"transaction\",";
        response += "\"target\":\"" + address + "\",";
        response += "\"type\":\"" + type + "\",";
        response += "\"message\":\"Transaction sent to " + address + "\"";
      }
      if (gateway_forwarded) {
        response += ",\"internet_forwarded\":true,";
        response += "\"internet_response\":\"" + escapeJsonString(internetResponse) + "\"";
      }
    } else {
      response += "\"success\":false,";
      response += "\"error\":\"Invalid address format. Use REGION.COMMUNITY.NODE\"";
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"No transaction data provided\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiBroadcast() {
  addLog("[API] Broadcast endpoint called - IP: " + server.client().remoteIP().toString());
  addLog("[API] LoRa state - isLoRaIdle: " + String(isLoRaIdle ? "true" : "false"));
  addLog("[API] Request state - " + String(currentRequestState));
  
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("message")) {
    String message = server.arg("message");
    String type = server.hasArg("type") ? server.arg("type") : "announcement";
    String priority = server.hasArg("priority") ? server.arg("priority") : "normal";
    
    // Check if system is busy
    if (currentRequestState != REQUEST_IDLE) {
      // Queue the request
      if (QueueRequest(REQUEST_BROADCAST, message, type, priority)) {
        response += "\"action\":\"broadcast_queued\",";
        response += "\"type\":\"" + type + "\",";
        response += "\"priority\":\"" + priority + "\",";
        response += "\"message\":\"Broadcast queued - will be sent when system is available\",";
        response += "\"queue_size\":" + String(pendingRequestCount);
        addLog("[API] Broadcast request queued - Queue size: " + String(pendingRequestCount));
      } else {
        response += "\"success\":false,";
        response += "\"error\":\"Request queue is full - please try again later\"";
      }
    } else {
      // Process immediately
      if (QueueRequest(REQUEST_BROADCAST, message, type, priority)) {
        ProcessNextQueuedRequest();
        response += "\"action\":\"broadcast\",";
        response += "\"type\":\"" + type + "\",";
        response += "\"priority\":\"" + priority + "\",";
        response += "\"message\":\"Broadcast sent to all devices\"";
        addLog("[API] Broadcast request processed immediately");
      } else {
        response += "\"success\":false,";
        response += "\"error\":\"Failed to queue broadcast request\"";
      }
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"No broadcast message provided\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiStatus() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"device\":{";
  response += "\"name\":\"RadioDoge\",";
  response += "\"address\":\"" + String(local.region) + "." + String(local.community) + "." + String(local.node) + "\",";
  response += "\"status\":\"Ready\",";
  response += "\"lora\":\"Active\",";
  response += "\"wifi\":\"" + String(ap_ssid) + "\",";
  response += "\"uptime\":" + String(millis() / 1000) + ",";
  response += "\"free_memory\":" + String(ESP.getFreeHeap());
  response += "},";
  response += "\"request_queue\":{";
  response += "\"state\":\"" + String(currentRequestState) + "\",";
  response += "\"pending_count\":" + String(pendingRequestCount) + ",";
  response += "\"max_requests\":" + String(MAX_PENDING_REQUESTS) + ",";
  response += "\"current_request_id\":\"" + currentRequestId + "\"";
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiQueueStatus() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"queue\":{";
  response += "\"state\":\"" + String(currentRequestState) + "\",";
  response += "\"pending_count\":" + String(pendingRequestCount) + ",";
  response += "\"max_requests\":" + String(MAX_PENDING_REQUESTS) + ",";
  response += "\"current_request_id\":\"" + currentRequestId + "\",";
  response += "\"confirmation_timeout_ms\":" + String(CONFIRMATION_TIMEOUT_MS) + ",";
  response += "\"request_timeout_ms\":" + String(REQUEST_TIMEOUT_MS) + ",";
  response += "\"requests\":[";
  
  for (int i = 0; i < pendingRequestCount; i++) {
    if (i > 0) response += ",";
    response += "{";
    response += "\"id\":\"" + pendingRequests[i].requestId + "\",";
    response += "\"type\":" + String(pendingRequests[i].type) + ",";
    response += "\"type_str\":\"" + pendingRequests[i].typeStr + "\",";
    response += "\"priority\":\"" + pendingRequests[i].priority + "\",";
    response += "\"timestamp\":" + String(pendingRequests[i].timestamp) + ",";
    response += "\"is_multipart\":" + String(pendingRequests[i].isMultipart ? "true" : "false") + ",";
    response += "\"requires_confirmation\":" + String(pendingRequests[i].requiresConfirmation ? "true" : "false") + ",";
    response += "\"message_length\":" + String(pendingRequests[i].message.length());
    response += "}";
  }
  
  response += "]";
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiAddress() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.method() == HTTP_POST) {
    // Set new address
    if (server.hasArg("region") && server.hasArg("community") && server.hasArg("node")) {
      nodeAddress requested;
      if (!parseLocalAddressOctets(server.arg("region").toInt(),
                                   server.arg("community").toInt(),
                                   server.arg("node").toInt(), requested)) {
        response += "\"success\":false,";
        response += "\"error\":\"Invalid address: each octet must be 0-255, and 255.255.255 is reserved for broadcast\"";
        response += "}";
        server.send(200, "application/json", response);
        return;
      }
      local = requested;
      InitControlMessages();
      DisplayLocalAddress(local);
      // Save LoRa configuration to NVS
      saveLoRaConfiguration(local.region, local.community, local.node);
      
      response += "\"action\":\"set_address\",";
      response += "\"address\":\"" + String(local.region) + "." + String(local.community) + "." + String(local.node) + "\",";
      response += "\"message\":\"Address updated and saved successfully\"";
    } else {
      response += "\"success\":false,";
      response += "\"error\":\"Missing required parameters: region, community, node\"";
    }
  } else {
    // Get current address
    response += "\"action\":\"get_address\",";
    response += "\"address\":\"" + String(local.region) + "." + String(local.community) + "." + String(local.node) + "\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// WiFi Configuration API Handlers
void handleApiWifi() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"wifi\":{";
  response += "\"ap_ssid\":\"" + String(ap_ssid) + "\",";
  response += "\"ap_ip\":\"" + WiFi.softAPIP().toString() + "\",";
  response += "\"internet_connected\":" + String(internet_connected ? "true" : "false") + ",";
  response += "\"internet_ssid\":\"" + escapeJsonString(internet_ssid) + "\",";
  if (internet_connected) {
    response += "\"internet_ip\":\"" + WiFi.localIP().toString() + "\",";
  }
  response += "\"dual_mode\":" + String(dual_wifi_mode ? "true" : "false");
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiWifiConnect() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("ssid") && server.hasArg("password")) {
    internet_ssid = server.arg("ssid");
    internet_password = server.arg("password");
    
    // Attempt to connect
    connectToInternetWiFi();
    
    if (internet_connected) {
      response += "\"action\":\"wifi_connect\",";
      response += "\"message\":\"Successfully connected to " + internet_ssid + "\",";
      response += "\"ip\":\"" + WiFi.localIP().toString() + "\"";
    } else {
      response += "\"success\":false,";
      response += "\"action\":\"wifi_connect_failed\",";
      response += "\"error\":\"Failed to connect to " + internet_ssid + "\"";
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"Missing SSID or password\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiWifiDisconnect() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  disconnectInternetWiFi();
  
  response += "\"action\":\"wifi_disconnect\",";
  response += "\"message\":\"Disconnected from internet WiFi\"";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiWifiClear() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  clearWiFiCredentials();
  
  response += "\"action\":\"wifi_clear\",";
  response += "\"message\":\"WiFi credentials cleared from storage\"";
  response += "}";
  server.send(200, "application/json", response);
}

// Internet Bridge API Handlers
void handleApiBridgeEnable() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  enableInternetBridge();
  
  response += "\"action\":\"bridge_enable\",";
  response += "\"bridge_enabled\":" + String(internet_bridge_enabled ? "true" : "false") + ",";
  response += "\"internet_connected\":" + String(internet_connected ? "true" : "false") + ",";
  response += "\"message\":\"Internet bridge " + String(internet_bridge_enabled ? "enabled" : "failed to enable") + "\"";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiBridgeDisable() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  disableInternetBridge();
  
  response += "\"action\":\"bridge_disable\",";
  response += "\"bridge_enabled\":" + String(internet_bridge_enabled ? "true" : "false") + ",";
  response += "\"message\":\"Internet bridge disabled\"";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiBridgeStatus() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"bridge\":{";
  response += "\"enabled\":" + String(internet_bridge_enabled ? "true" : "false") + ",";
  response += "\"internet_connected\":" + String(internet_connected ? "true" : "false") + ",";
  response += "\"ap_gateway\":\"" + ap_gateway.toString() + "\",";
  response += "\"ap_subnet\":\"" + ap_subnet.toString() + "\",";
  response += "\"internet_ip\":\"" + (internet_connected ? WiFi.localIP().toString() : "none") + "\",";
  response += "\"internet_ssid\":\"" + escapeJsonString(internet_ssid) + "\"";
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

// HTTP Proxy for Internet Bridge
// Largest proxied response this device will hold in RAM.
//
// v0.4.2 — There was no limit: the whole remote body went into an Arduino String
// on the heap. The board has a couple of hundred KB free with WiFi, BLE and the
// web server running, so a single request for any ordinary web page was enough
// to exhaust it and reset the device mid-transaction. 32 KB is generous for the
// status pages this is meant to fetch and small enough to be safe.
#define PROXY_MAX_RESPONSE_BYTES 32768

void handleHttpProxy() {
  // v0.4.2 — Gated on the operator having explicitly enabled the bridge, not
  // merely on the board happening to have internet.
  //
  // This route is an open forward proxy: it will fetch any URL and return the
  // body, so while the STA interface is up, anything that can reach this web
  // server can reach the upstream network through it — the operator's router
  // admin page, their NAS, anything else on that LAN. The bridge already has an
  // explicit on/off switch and an API for it; requiring it here means the
  // capability exists only while it has been asked for, instead of whenever the
  // board is online.
  if (!internet_bridge_enabled) {
    server.send(403, "text/plain",
                "The internet bridge is disabled. Enable it (POST /api/bridge/enable) to use the proxy.");
    return;
  }
  if (!internet_connected) {
    server.send(503, "text/plain", "Internet not connected");
    return;
  }

  if (!server.hasArg("url")) {
    server.send(400, "text/plain", "Missing 'url' parameter. Usage: /proxy?url=http://example.com");
    return;
  }

  String url = server.arg("url");
  HTTPClient http;

  // Add http:// if not present
  if (!url.startsWith("http://") && !url.startsWith("https://")) {
    url = "http://" + url;
  }

  http.begin(url);
  http.setTimeout(10000); // 10 second timeout

  int httpCode = http.GET();
  if (httpCode <= 0) {
    server.send(500, "text/plain", "Error: " + String(httpCode));
    http.end();
    return;
  }

  // Refuse anything that declares itself too large before reading a byte of it.
  int declaredSize = http.getSize();
  if (declaredSize > (int)PROXY_MAX_RESPONSE_BYTES) {
    server.send(502, "text/plain",
                "Response too large to proxy (" + String(declaredSize) + " bytes, limit " +
                String(PROXY_MAX_RESPONSE_BYTES) + ")");
    http.end();
    return;
  }

  // A chunked response declares no size, so read with a hard ceiling rather than
  // trusting the header.
  WiFiClient *stream = http.getStreamPtr();
  String response;
  response.reserve(declaredSize > 0 ? min(declaredSize, (int)PROXY_MAX_RESPONSE_BYTES) : 1024);
  uint8_t buf[512];
  size_t total = 0;
  unsigned long lastData = millis();
  while (http.connected() && total < PROXY_MAX_RESPONSE_BYTES) {
    size_t avail = stream->available();
    if (avail == 0) {
      if (millis() - lastData > 10000) break;  // stalled
      delay(1);
      continue;
    }
    size_t want = min(avail, sizeof(buf));
    if (total + want > PROXY_MAX_RESPONSE_BYTES) {
      want = PROXY_MAX_RESPONSE_BYTES - total;
    }
    int got = stream->readBytes(buf, want);
    if (got <= 0) break;
    for (int i = 0; i < got; i++) {
      response += (char)buf[i];
    }
    total += got;
    lastData = millis();
  }

  if (total >= PROXY_MAX_RESPONSE_BYTES) {
    addLog("[PROXY] Response truncated at " + String(PROXY_MAX_RESPONSE_BYTES) + " bytes: " + url);
  }
  server.send(httpCode, "text/html", response);
  http.end();
}

void handleApiLoRaClear() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  clearLoRaConfiguration();
  InitControlMessages();
  
  response += "\"action\":\"lora_clear\",";
  response += "\"message\":\"LoRa configuration cleared from storage\",";
  response += "\"new_address\":\"" + String(local.region) + "." + String(local.community) + "." + String(local.node) + "\"";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiPasswordChange() {
  bool restartAfterReply = false;
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("password")) {
    String newPassword = server.arg("password");
    
    // Validate password
    if (!validatePassword(newPassword)) {
      response += "\"success\":false,";
      response += "\"error\":\"Password does not meet requirements. Must be 8-32 characters with letters and numbers\"";
    } else {
      // Save new password
      ap_password = newPassword;
      saveAPPassword(newPassword);
      response += "\"action\":\"password_change\",";
      response += "\"message\":\"Password changed. The access point is restarting with the new password — rejoin it to continue.\"";
      restartAfterReply = true;
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"No password provided\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);

  // v0.4.2 — restart AFTER replying. restartAP() calls softAPdisconnect(), which
  // drops every client including the one waiting for this response, so the UI
  // saw the request hang and could not tell success from failure.
  if (restartAfterReply) {
    restartAP();
  }
}

void handleApiPasswordReset() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  // Clear stored password and reset to default
  clearAPPassword();

  response += "\"action\":\"password_reset\",";
  response += "\"message\":\"Password reset to the default. The access point is restarting — rejoin it to continue.\"";
  response += "}";
  server.send(200, "application/json", response);

  // v0.4.2 — restart after replying; see handleApiPasswordChange.
  restartAP();
}

void handleApiPasswordStatus() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"password\":{";
  // SECURITY: the AP password is deliberately NOT returned. This route is
  // unauthenticated, so echoing it handed the credential to anyone who could
  // reach the board. The fields below tell the UI everything it needs (whether
  // the default is still in use, and the length) without disclosing the secret.
  response += "\"is_default\":" + String(ap_password == "radiodoge" ? "true" : "false") + ",";
  response += "\"length\":" + String(ap_password.length()) + ",";
  response += "\"requirements\":{";
  response += "\"min_length\":" + String(MIN_PASSWORD_LENGTH) + ",";
  response += "\"max_length\":" + String(MAX_PASSWORD_LENGTH) + ",";
  response += "\"needs_letter_number\":true";
  response += "}";
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

void handleApiGateway() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (!internet_connected) {
    response += "\"success\":false,";
    response += "\"error\":\"No internet connection available\"";
  } else if (server.hasArg("transaction")) {
    String transaction = server.arg("transaction");
    String gatewayUrl = server.hasArg("url") ? server.arg("url") : "https://api.blockcypher.com/v1/doge/main/txs/push";
    
    String serverResponse = sendTransactionToCustomGateway(transaction, gatewayUrl);
    
    response += "\"action\":\"gateway_send\",";
    response += "\"message\":\"Transaction sent to gateway\",";
    response += "\"gateway\":\"" + gatewayUrl + "\",";
    response += "\"server_response\":\"" + escapeJsonString(serverResponse) + "\"";
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"No transaction data provided\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Status API Handler
void handleApiGatewayStatus() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"gateway\":{";
  response += "\"type\":\"" + escapeJsonString(gateway_type) + "\",";
  response += "\"ip\":\"" + escapeJsonString(gateway_ip) + "\",";
  response += "\"port\":\"" + escapeJsonString(gateway_port) + "\",";
  response += "\"endpoint\":\"" + escapeJsonString(gateway_endpoint) + "\",";
  response += "\"username\":\"" + escapeJsonString(gateway_username) + "\",";
  response += "\"password\":\"";
  response += (gateway_password.length() > 0 ? "***" : "");
  response += "\"";
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Save API Handler
void handleApiGatewaySave() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("type") && server.hasArg("ip") && server.hasArg("port")) {
    String type = server.arg("type");
    String ip = server.arg("ip");
    String port = server.arg("port");
    String endpoint = server.hasArg("endpoint") ? server.arg("endpoint") : "";
    String username = server.hasArg("username") ? server.arg("username") : "";
    String password = server.hasArg("password") ? server.arg("password") : "";

    // The gateway password is write-only: /api/gateway/load no longer returns it,
    // so the UI's password field arrives blank unless the operator typed a new
    // one. Treat blank as "keep the stored password" — otherwise simply saving
    // an unrelated setting would silently erase the credential.
    if (password.length() == 0) {
      password = gateway_password;
    }

    addLog("[API] Gateway save request - Type: " + type + ", IP: " + ip + ", Port: " + port + ", Username: [REDACTED], Password: [REDACTED]");
    
    // Validate required fields
    if (type == "none") {
      response += "\"success\":false,";
      response += "\"error\":\"Please select a gateway type\"";
    } else if (ip.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"Please enter IP address\"";
    } else if (port.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"Please enter port\"";
    } else if (type != "core" && endpoint.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"Please enter endpoint for this gateway type\"";
    } else if (type == "core" && (username.length() == 0 || password.length() == 0)) {
      response += "\"success\":false,";
      response += "\"error\":\"Please enter RPC username and password for CORE gateway\"";
    } else {
      // Save credentials
      saveGatewayCredentials(type, ip, port, endpoint, username, password);
      
      // Update global variables
      gateway_type = type;
      gateway_ip = ip;
      gateway_port = port;
      gateway_endpoint = endpoint;
      gateway_username = username;
      gateway_password = password;

      // v0.4.1 — Forwarding now requires gateway_mode (see gatewayForwardingEnabled).
      // Configuring a gateway here is an explicit statement of intent to act as
      // one, so enable and persist the mode. Without this, a board set up purely
      // through the web UI would silently stop forwarding after the upgrade.
      if (type != "none" && !gateway_mode) {
        gateway_mode = true;
        saveGatewayModeQuiet(true);
        addLog("[GATEWAY] Gateway configured - gateway mode enabled");
      }
      
      response += "\"action\":\"gateway_save\",";
      response += "\"message\":\"Gateway credentials saved successfully\"";
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"Missing required parameters\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Clear API Handler
void handleApiGatewayClear() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  clearGatewayCredentials();
  
  // Reset global variables
  gateway_type = "none";
  gateway_ip = "";
  gateway_port = "";
  gateway_endpoint = "";
  gateway_username = "";
  gateway_password = "";
  
  response += "\"action\":\"gateway_clear\",";
  response += "\"message\":\"Gateway credentials cleared successfully\"";
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Debug API Handler
void handleApiGatewayDebug() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  nvs_handle_t nvs_handle;
  esp_err_t err = nvs_open("gateway_config", NVS_READONLY, &nvs_handle);
  
  if (err != ESP_OK) {
    response += "\"error\":\"Failed to open NVS handle: " + String(err) + "\"";
  } else {
    response += "\"nvs_status\":\"opened\",";
    
    // Check each key
    size_t type_len = 16;
    char type_buffer[16];
    err = nvs_get_str(nvs_handle, "gateway_type", type_buffer, &type_len);
    response += "\"gateway_type_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gateway_type_error\":" + String(err) + ",";
    
    size_t ip_len = 16;
    char ip_buffer[16];
    err = nvs_get_str(nvs_handle, "gateway_ip", ip_buffer, &ip_len);
    response += "\"gateway_ip_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gateway_ip_error\":" + String(err) + ",";
    
    size_t port_len = 8;
    char port_buffer[8];
    err = nvs_get_str(nvs_handle, "gateway_port", port_buffer, &port_len);
    response += "\"gateway_port_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gateway_port_error\":" + String(err) + ",";
    
    size_t endpoint_len = 64;
    char endpoint_buffer[64];
    err = nvs_get_str(nvs_handle, NVS_KEY_GATEWAY_ENDPOINT, endpoint_buffer, &endpoint_len);
    response += "\"gateway_endpoint_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gateway_endpoint_error\":" + String(err) + ",";
    
    size_t username_len = 128;
    char username_buffer[128];
    err = nvs_get_str(nvs_handle, "gateway_user", username_buffer, &username_len);
    response += "\"gateway_user_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gateway_user_error\":" + String(err) + ",";
    
    size_t password_len = 256;
    char password_buffer[256];
    err = nvs_get_str(nvs_handle, "gateway_pass", password_buffer, &password_len);
    response += "\"gateway_pass_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gateway_pass_error\":" + String(err) + ",";
    
    // Also check the new key name
    password_len = 256;
    err = nvs_get_str(nvs_handle, "gateway_password", password_buffer, &password_len);
    response += "\"gateway_password_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gateway_password_error\":" + String(err) + ",";
    
    // Check fallback key name
    password_len = 256;
    err = nvs_get_str(nvs_handle, "gw_pass", password_buffer, &password_len);
    response += "\"gw_pass_exists\":" + String(err == ESP_OK ? "true" : "false") + ",";
    response += "\"gw_pass_error\":" + String(err) + ",";
    
    nvs_close(nvs_handle);
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Load API Handler (returns actual password for web interface)
void handleApiGatewayLoad() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"gateway\":{";
  response += "\"type\":\"" + escapeJsonString(gateway_type) + "\",";
  response += "\"ip\":\"" + escapeJsonString(gateway_ip) + "\",";
  response += "\"port\":\"" + escapeJsonString(gateway_port) + "\",";
  response += "\"endpoint\":\"" + escapeJsonString(gateway_endpoint) + "\",";
  response += "\"username\":\"" + escapeJsonString(gateway_username) + "\",";
  // SECURITY: the stored gateway RPC password is deliberately NOT returned.
  // This route is unauthenticated, so returning it disclosed the credential to
  // anyone on the board's AP. The password is write-only: the UI reports
  // whether one is stored and leaves the field blank, and an operator who wants
  // to change it types a new one.
  response += "\"has_password\":" + String(gateway_password.length() > 0 ? "true" : "false");
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Test API Handler
void handleApiGatewayTest() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (!internet_connected) {
    response += "\"success\":false,";
    response += "\"error\":\"No internet connection available\"";
  } else if (server.hasArg("url")) {
    String gatewayUrl = server.arg("url");
    
    // Simple HTTP GET test to check if gateway is reachable
    HTTPClient http;
    http.begin(gatewayUrl);
    http.setTimeout(5000); // 5 second timeout
    
    int httpResponseCode = http.GET();
    String serverResponse = "";
    
    if (httpResponseCode > 0) {
      serverResponse = http.getString();
      response += "\"action\":\"gateway_test\",";
      response += "\"message\":\"Gateway test completed\",";
      response += "\"gateway\":\"" + gatewayUrl + "\",";
      response += "\"http_code\":" + String(httpResponseCode) + ",";
      response += "\"server_response\":\"" + escapeJsonString(serverResponse) + "\"";
    } else {
      response += "\"success\":false,";
      response += "\"error\":\"Gateway not reachable (HTTP " + String(httpResponseCode) + ")\"";
    }
    
    http.end();
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"No gateway URL provided\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// New RPC handler for Dogecoin Core sendrawtransaction
void handleApiRpc() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (!internet_connected) {
    response += "\"success\":false,";
    response += "\"error\":\"No internet connection available\"";
  } else if (server.hasArg("body")) {
    String rpcBody = server.arg("body");
    String gatewayUrl = server.hasArg("url") ? server.arg("url") : "";
    
    if (gatewayUrl.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"No gateway URL provided\"";
    } else {
      bool success = sendRpcToGateway(rpcBody, gatewayUrl);
      
      if (success) {
        response += "\"action\":\"rpc_send\",";
        response += "\"message\":\"RPC call sent to gateway successfully\",";
        response += "\"gateway\":\"" + gatewayUrl + "\"";
      } else {
        response += "\"success\":false,";
        response += "\"error\":\"Failed to send RPC call to gateway\"";
      }
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"No RPC body provided\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// New JSON-RPC handler for Dogecoin Core sendrawtransaction
void handleApiJsonRpc() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (!internet_connected) {
    response += "\"success\":false,";
    response += "\"error\":\"No internet connection available\"";
  } else if (server.hasArg("transaction") && server.hasArg("url") && server.hasArg("rpcuser") && server.hasArg("rpcpass")) {
    String transaction = server.arg("transaction");
    String gatewayUrl = server.arg("url");
    String rpcUser = server.arg("rpcuser");
    String rpcPass = server.arg("rpcpass");
    
    String serverResponse = sendJsonRpcToGateway(transaction, gatewayUrl, rpcUser, rpcPass);
    String parsedResponse = parseDogecoinCoreResponse(serverResponse);
    
    response += "\"action\":\"jsonrpc_send\",";
    response += "\"message\":\"JSON-RPC call sent to Dogecoin Core\",";
    response += "\"gateway\":\"" + gatewayUrl + "\",";
    response += "\"server_response\":\"" + escapeJsonString(serverResponse) + "\",";
    response += "\"parsed_response\":" + parsedResponse;
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"Missing required parameters: transaction, url, rpcuser, rpcpass\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// New function to send RPC calls to Dogecoin Core
bool sendRpcToGateway(String rpcBody, String gatewayUrl) {
  if (!internet_connected) {
    debugPrintln("No internet connection available");
    return false;
  }
  
  HTTPClient http;
  http.begin(gatewayUrl);
  http.addHeader("Content-Type", "application/x-www-form-urlencoded");
  
  int httpResponseCode = http.POST(rpcBody);
  
  if (httpResponseCode > 0) {
    String response = http.getString();
    debugPrintln("RPC call sent to gateway");
    debugPrintln("Response: " + response);
    http.end();
    return true;
  } else {
    debugPrintln("Error sending RPC call to gateway: " + String(httpResponseCode));
    http.end();
    return false;
  }
}

// Logs API Handler
void handleApiLogs() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"action\":\"logs_retrieve\",";
  response += "\"total_logs\":" + String(logCount) + ",";
  response += "\"logs\":[";
  
  if (logCount > 0) {
    int startIndex = (logCount < MAX_LOG_ENTRIES) ? 0 : logIndex;
    int entriesToShow = min(logCount, MAX_LOG_ENTRIES);
    
    for (int i = 0; i < entriesToShow; i++) {
      int actualIndex = (startIndex + i) % MAX_LOG_ENTRIES;
      if (i > 0) response += ",";
      response += "\"" + escapeJsonString(logBuffer[actualIndex]) + "\"";
    }
  }
  
  response += "]}";
  server.send(200, "application/json", response);
}

// Plain text logs API handler
void handleApiLogsText() {
  String response = "";
  
  if (logCount > 0) {
    int startIndex = (logCount < MAX_LOG_ENTRIES) ? 0 : logIndex;
    int entriesToShow = min(logCount, MAX_LOG_ENTRIES);
    
    for (int i = 0; i < entriesToShow; i++) {
      int actualIndex = (startIndex + i) % MAX_LOG_ENTRIES;
      response += logBuffer[actualIndex] + "\n";
    }
  } else {
    response = "No logs available\n";
  }
  
  server.send(200, "text/plain", response);
}

// Logs Send API Handler - Send logs to other devices via LoRa
void handleApiLogsSend() {
  addLog("[API] Logs send endpoint called - IP: " + server.client().remoteIP().toString());
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("address") && server.hasArg("logs")) {
    String address = server.arg("address");
    String logs = server.arg("logs");
    String type = server.hasArg("type") ? server.arg("type") : "logs";
    
    // Parse address
    int dots[2];
    int dotCount = 0;
    for (int i = 0; i < address.length() && dotCount < 2; i++) {
      if (address.charAt(i) == '.') {
        dots[dotCount] = i;
        dotCount++;
      }
    }
    
    if (dotCount == 2) {
      int region = address.substring(0, dots[0]).toInt();
      int community = address.substring(dots[0] + 1, dots[1]).toInt();
      int node = address.substring(dots[1] + 1).toInt();
      
      nodeAddress logDest = {region, community, node};
      
      // Send logs as a message
      SendMessage(logDest, logs, type);
      
      addLog("[API] Sending logs via LoRa - To: " + address + ", Type: " + type + ", Length: " + String(logs.length()));
      
      response += "\"action\":\"logs_send\",";
      response += "\"target\":\"" + address + "\",";
      response += "\"type\":\"" + type + "\",";
      response += "\"message\":\"Logs sent to " + address + "\"";
    } else {
      response += "\"success\":false,";
      response += "\"error\":\"Invalid address format. Use region.community.node\"";
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"Missing required parameters: address and logs\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// Transaction Send API Handler - Send transaction using stored gateway credentials
void handleApiTransactionSend() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (!internet_connected) {
    response += "\"success\":false,";
    response += "\"error\":\"No internet connection available\"";
  } else if (server.hasArg("transaction")) {
    String transaction = server.arg("transaction");
    
    if (gateway_type == "none" || gateway_ip.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"No gateway configured. Please configure a gateway first.\"";
    } else {
      String gatewayUrl = "http://" + gateway_ip + ":" + gateway_port;
      if (gateway_type != "core" && gateway_endpoint.length() > 0) {
        gatewayUrl += gateway_endpoint;
      }
      
      String serverResponse = "";
      
      if (gateway_type == "core") {
        // Use JSON-RPC for Dogecoin Core
        String rawResponse = sendJsonRpcToGateway(transaction, gatewayUrl, gateway_username, gateway_password);
        serverResponse = parseDogecoinCoreResponse(rawResponse);
        addLog("Transaction sent to Dogecoin Core via JSON-RPC");
      } else {
        // Use regular HTTP POST for other gateways
        serverResponse = sendTransactionToCustomGateway(transaction, gatewayUrl);
        addLog("Transaction sent to " + gateway_type + " gateway");
      }
      
      response += "\"action\":\"transaction_send\",";
      response += "\"message\":\"Transaction sent to stored gateway\",";
      response += "\"gateway_type\":\"" + escapeJsonString(gateway_type) + "\",";
      response += "\"gateway_url\":\"" + gatewayUrl + "\",";
      response += "\"server_response\":\"" + escapeJsonString(serverResponse) + "\"";
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"Missing required parameter: transaction\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Config API Handler - Get current gateway configuration
void handleApiGatewayConfig() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  response += "\"action\":\"gateway_config_get\",";
  response += "\"config\":{";
  response += "\"type\":\"" + escapeJsonString(gateway_type) + "\",";
  response += "\"ip\":\"" + escapeJsonString(gateway_ip) + "\",";
  response += "\"port\":\"" + escapeJsonString(gateway_port) + "\",";
  response += "\"endpoint\":\"" + escapeJsonString(gateway_endpoint) + "\",";
  response += "\"username\":\"" + escapeJsonString(gateway_username) + "\",";
  response += "\"password\":\"";
  if (gateway_password.length() > 0) {
    response += "***";
  }
  response += "\"";
  response += "}";
  response += "}";
  server.send(200, "application/json", response);
}

// Gateway Config Set API Handler - Set gateway configuration
void handleApiGatewayConfigSet() {
  String response = "{";
  response += "\"success\":true,";
  response += "\"timestamp\":" + String(millis()) + ",";
  
  if (server.hasArg("type") && server.hasArg("ip") && server.hasArg("port")) {
    String type = server.arg("type");
    String ip = server.arg("ip");
    String port = server.arg("port");
    String endpoint = server.hasArg("endpoint") ? server.arg("endpoint") : "";
    String username = server.hasArg("username") ? server.arg("username") : "";
    String password = server.hasArg("password") ? server.arg("password") : "";
    
    // Validate required fields
    if (type == "none") {
      response += "\"success\":false,";
      response += "\"error\":\"Invalid gateway type\"";
    } else if (ip.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"IP address is required\"";
    } else if (port.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"Port is required\"";
    } else if (type != "core" && endpoint.length() == 0) {
      response += "\"success\":false,";
      response += "\"error\":\"Endpoint is required for non-CORE gateways\"";
    } else if (type == "core" && (username.length() == 0 || password.length() == 0)) {
      response += "\"success\":false,";
      response += "\"error\":\"Username and password are required for CORE gateway\"";
    } else {
      // Save the configuration
      saveGatewayCredentials(type, ip, port, endpoint, username, password);
      
      // Update global variables
      gateway_type = type;
      gateway_ip = ip;
      gateway_port = port;
      gateway_endpoint = endpoint;
      gateway_username = username;
      gateway_password = password;

      // v0.4.1 — Forwarding now requires gateway_mode (see gatewayForwardingEnabled).
      // Configuring a gateway here is an explicit statement of intent to act as
      // one, so enable and persist the mode. Without this, a board set up purely
      // through the web UI would silently stop forwarding after the upgrade.
      if (type != "none" && !gateway_mode) {
        gateway_mode = true;
        saveGatewayModeQuiet(true);
        addLog("[GATEWAY] Gateway configured - gateway mode enabled");
      }
      
      addLog("Gateway configuration updated via API");
      
      response += "\"action\":\"gateway_config_set\",";
      response += "\"message\":\"Gateway configuration updated successfully\"";
    }
  } else {
    response += "\"success\":false,";
    response += "\"error\":\"Missing required parameters: type, ip, port\"";
  }
  
  response += "}";
  server.send(200, "application/json", response);
}

// Function to send proper JSON-RPC calls to Dogecoin Core
String sendJsonRpcToGateway(String transaction, String gatewayUrl, String rpcUser, String rpcPass) {
  if (!internet_connected) {
    debugPrintln("No internet connection available");
    return "{\"error\":\"No internet connection available\"}";
  }
  
  debugPrintln("=== JSON-RPC Debug Info ===");
  debugPrintln("Gateway URL: " + gatewayUrl);
  debugPrintln("RPC User: [REDACTED]");
  debugPrintln("Transaction length: " + String(transaction.length()));
  
  HTTPClient http;
  http.setTimeout(10000); // 10 second timeout
  http.begin(gatewayUrl);
  http.addHeader("Content-Type", "application/json");
  http.setAuthorization(rpcUser.c_str(), rpcPass.c_str());
  
  // Create proper JSON-RPC payload
  String jsonPayload = "{\"jsonrpc\":\"1.0\",\"id\":\"curl\",\"method\":\"sendrawtransaction\",\"params\":[\"" + transaction + "\"]}";
  debugPrintln("JSON Payload: " + String(jsonPayload.length()) + " bytes");
  
  int httpResponseCode = http.POST(jsonPayload);
  String response = "";
  
  debugPrintln("HTTP Response Code: " + String(httpResponseCode));
  
  if (httpResponseCode > 0) {
    response = http.getString();
    debugPrintln("Raw Response: " + response);
    
    if (response.length() == 0) {
      response = "{\"error\":\"Empty response from server (HTTP " + String(httpResponseCode) + ")\"}";
    }
  } else {
    response = "{\"error\":\"HTTP Error " + String(httpResponseCode) + " - " + http.errorToString(httpResponseCode) + "\"}";
    debugPrintln("Error sending JSON-RPC call to gateway: " + String(httpResponseCode) + " - " + http.errorToString(httpResponseCode));
  }
  
  http.end();
  debugPrintln("=== End JSON-RPC Debug ===");
  return response;
}

// Parse Dogecoin Core JSON-RPC response to extract transaction ID or error
String parseDogecoinCoreResponse(String jsonResponse) {
  // Expected success response: {"result":"48801f8e4def6e8e427de4a89d6b40dda46998916729a051b086d70646bce0b8","error":null,"id":"curl"}
  // Expected error response: {"result":null,"error":{"code":-27,"message":"transaction already in block chain"},"id":"curl"}
  
  String result = "";
  String error = "";
  
  // Simple JSON parsing - look for "result" and "error" fields
  int resultStart = jsonResponse.indexOf("\"result\":\"");
  int errorStart = jsonResponse.indexOf("\"error\":{");
  
  if (resultStart != -1) {
    // Extract transaction ID from result field
    int resultValueStart = resultStart + 10; // Skip "result":"
    int resultValueEnd = jsonResponse.indexOf("\"", resultValueStart);
    if (resultValueEnd != -1) {
      result = jsonResponse.substring(resultValueStart, resultValueEnd);
    }
  }
  
  if (errorStart != -1) {
    // Extract error message
    int messageStart = jsonResponse.indexOf("\"message\":\"", errorStart);
    if (messageStart != -1) {
      int messageValueStart = messageStart + 11; // Skip "message":"
      int messageValueEnd = jsonResponse.indexOf("\"", messageValueStart);
      if (messageValueEnd != -1) {
        error = jsonResponse.substring(messageValueStart, messageValueEnd);
      }
    }
  }
  
  // Return parsed response
  if (!result.isEmpty() && result != "null") {
    return "{\"success\":true,\"transaction_id\":\"" + result + "\",\"raw_response\":\"" + escapeJsonString(jsonResponse) + "\"}";
  } else if (!error.isEmpty()) {
    return "{\"success\":false,\"error\":\"" + error + "\",\"raw_response\":\"" + escapeJsonString(jsonResponse) + "\"}";
  } else {
    return "{\"success\":false,\"error\":\"Unknown response format\",\"raw_response\":\"" + escapeJsonString(jsonResponse) + "\"}";
  }
}