# API contract tests with Hurl (validated against api/openapi.yaml).
# Run with: task test:api   (boots the server, then runs this file)

# Health
GET {{host}}/health
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.status" == "ok"

# Canvas shape
GET {{host}}/api/canvas
HTTP 200
[Asserts]
jsonpath "$.width" == 64
jsonpath "$.height" == 64
jsonpath "$.palette" count == 16
jsonpath "$.pixels" isString

# Paint a valid pixel
POST {{host}}/api/pixel
Content-Type: application/json
{ "x": 0, "y": 0, "color": 5 }
HTTP 200
[Asserts]
jsonpath "$.ok" == true

# Reject an out-of-bounds pixel
POST {{host}}/api/pixel
Content-Type: application/json
{ "x": 999, "y": 0, "color": 5 }
HTTP 400
[Asserts]
jsonpath "$.ok" == false
jsonpath "$.error" isString

# Info exposes the serving instance (used to visualise load balancing)
GET {{host}}/info
HTTP 200
[Asserts]
jsonpath "$.name" == "pixelflux"
jsonpath "$.instance" isString
