#!/bin/bash

# Download data samples from the specified URL

# URL to download from
URL="https://www.mmsp.ece.mcgill.ca/Documents/AudioFormats/WAVE/Samples/AFsp/M1F1-AlawWE-AFsp.wav"

# Directory to save the files to
DIR="data"

# Create the directory if it doesn't exist
mkdir -p "$DIR"

# Download the file
curl -o "$DIR/sample.wav" "$URL"

# Print a success message
echo "Data samples downloaded to $DIR"