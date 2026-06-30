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
jsonpath "$.width" == 200
jsonpath "$.height" == 200
jsonpath "$.palette" count == 16
jsonpath "$.pixels" isString

# Register a token (deliberately slow: ~5s anti-abuse delay)
POST {{host}}/register
HTTP 200
[Captures]
token: jsonpath "$.token"
[Asserts]
jsonpath "$.token" isString

# Painting without a token is rejected
POST {{host}}/api/pixel
Content-Type: application/json
{ "x": 0, "y": 0, "color": 5 }
HTTP 401
[Asserts]
jsonpath "$.ok" == false

# Paint a valid pixel (authenticated with the token)
POST {{host}}/api/pixel
Content-Type: application/json
X-Token: {{token}}
{ "x": 0, "y": 0, "color": 5 }
HTTP 200
[Asserts]
jsonpath "$.ok" == true

# Reject an out-of-bounds pixel (authorised, but invalid coordinates)
POST {{host}}/api/pixel
Content-Type: application/json
X-Token: {{token}}
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
jsonpath "$.online" isInteger
