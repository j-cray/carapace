#!/usr/bin/env bash
set -e

API_URL="http://127.0.0.1:8080"
DEVICE_NAME="carapace"

echo "==> Checking if Signal API is running..."
if ! curl -s "$API_URL/v1/about" > /dev/null; then
    echo "Error: Signal API is not reachable at $API_URL."
    echo "Please ensure you have run ./run_local_with_signal.sh first and that it is fully initialized."
    exit 1
fi

echo "==> Requesting linking QR code for device '$DEVICE_NAME'..."

# Fetch the QR code link URI from the Signal API
RESPONSE=$(curl -s "$API_URL/v1/qrcodelink?device_name=$DEVICE_NAME")
URI=$(echo "$RESPONSE" | grep -o '"uri":"[^"]*' | grep -o '[^"]*$')

if [ -z "$URI" ]; then
    echo "Failed to get linking URI. Response was:"
    echo "$RESPONSE"
    exit 1
fi

echo ""
echo "========================================================="
echo "Please scan the QR code below using the Signal app on your phone."
echo "(Settings -> Linked Devices -> +)"
echo "========================================================="
echo ""

# Use qrenco.de to generate an ASCII QR code in the terminal
curl -s "https://qrenco.de/$URI"

echo ""
echo "========================================================="
echo "If scanning fails, you can try scanning this exact string:"
echo "$URI"
echo "========================================================="
echo ""
echo "After scanning and approving on your phone, wait a few seconds."
echo "Then check carapace's output. The '400 Bad Request' errors should stop,"
echo "and your new device will be ready to process messages!"
