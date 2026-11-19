// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

interface IToken {
    function transfer(address to, uint256 amount) external returns (bool);
}

library Math {
    function max(uint256 a, uint256 b) internal pure returns (uint256) {
        return a > b ? a : b;
    }
}

contract Vault is IToken {
    struct Deposit { uint256 amount; }
    enum State { Open, Closed }
    event Deposited(address indexed from, uint256 amount);
    error Unauthorized();

    modifier onlyOwner() {
        _;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        return true;
    }
}
