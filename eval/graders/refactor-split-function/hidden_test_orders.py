import ast
import random
import unittest
from pathlib import Path

import grader_original_orders as original
import orders

SOURCE = Path(orders.__file__).read_text()
REQUIRED = ["parse_orders", "validate_order", "compute_totals", "render_report", "process_orders"]
MAX_FUNCTION_LINES = 40
MAX_PROCESS_STATEMENTS = 12

FIXED_INPUTS = [
    "",
    "\n\n# only comments\n   \n",
    "o1,alice,MUG,2,7.50\n",
    "o1,alice,MUG,1,0.005\no2,alice,MUG,1,0.005\n",
    "o1,alice,MUG,3,19.999,SAVE10\no1,alice,TEA,10,2.10,BULK\no1,alice,PEN,9,2.10,BULK\n",
    "o1,alice,MUG,2,7.50\no1,alice,MUG,5,1.00\no2,alice,MUG,1,1.00\n",
    "o1,bob,MUG,2\no2,bob,MUG,2,1,SAVE10,extra\n,bob,X,1,1\no3,,X,1,1\n",
    "o1,carol,MUG,0,1\no1,carol,MUG,-3,1\no1,carol,MUG,2.5,1\no1,carol,MUG,x,abc\n",
    "o1,dave,MUG,1,-1\no1,dave,MUG,1,NaN\no1,dave,MUG,1,Infinity\no1,dave,MUG,1,1e2\n",
    "o1,erin,MUG,1,5,save10\no1,erin,TEA,1,5,FREE\no1,erin,PEN,1,5,\n",
    "  o9 , Zed , A , 4 , 2.25 , SAVE10  \r\no9,Zed,A,4,2.25\nO9,zed,a,1,1\n",
    "o1,bob,A,1,1\no2,alice,B,1,1\no3,Bob,C,1,1\no4,_x,D,1,1\n",
    "o1,x,A,,1\no1,x,A,1,\no1,x,A,1,1\n",
]

ATOMS = {
    "order_id": ["o1", "o2", "o3", "", "o4"],
    "customer": ["alice", "bob", "carol", "", "Dee"],
    "sku": ["MUG", "TEA", "PEN", "CUP"],
    "quantity": ["1", "2", "10", "12", "0", "-1", "two", "3"],
    "price": ["7.50", "0.99", "1.255", "100", "0", "-2", "abc", "12.345"],
    "coupon": [None, None, "", "SAVE10", "BULK", "BOGUS"],
}


def random_input(rng: random.Random) -> str:
    rows = []
    for _ in range(rng.randint(0, 25)):
        roll = rng.random()
        if roll < 0.05:
            rows.append("# comment")
        elif roll < 0.08:
            rows.append("")
        elif roll < 0.12:
            rows.append(",".join(rng.choice(ATOMS["sku"]) for _ in range(rng.choice([3, 4, 7]))))
        else:
            fields = [rng.choice(ATOMS[k]) for k in ("order_id", "customer", "sku", "quantity", "price")]
            coupon = rng.choice(ATOMS["coupon"])
            rows.append(",".join(fields + ([] if coupon is None else [coupon])))
    return "\n".join(rows) + ("\n" if rng.random() < 0.7 else "")


def module_functions(tree: ast.Module) -> dict[str, ast.FunctionDef]:
    return {node.name: node for node in tree.body if isinstance(node, ast.FunctionDef)}


def called_names(function: ast.AST) -> set[str]:
    return {node.func.id for node in ast.walk(function)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)}


class HiddenBehaviourTest(unittest.TestCase):
    def assert_same(self, text):
        self.assertEqual(orders.process_orders(text), original.process_orders(text), repr(text))

    def test_fixed_inputs_match_original(self):
        for text in FIXED_INPUTS:
            with self.subTest(text=text):
                self.assert_same(text)

    def test_random_inputs_match_original(self):
        rng = random.Random(20240601)
        for index in range(300):
            text = random_input(rng)
            with self.subTest(index=index):
                self.assert_same(text)


class HiddenStructureTest(unittest.TestCase):
    def setUp(self):
        self.tree = ast.parse(SOURCE)
        self.functions = module_functions(self.tree)

    def test_required_functions_exist(self):
        for name in REQUIRED:
            self.assertIn(name, self.functions, f"orders.py must define {name} at module level")
            self.assertTrue(callable(getattr(orders, name, None)))

    def test_no_long_functions(self):
        for node in ast.walk(self.tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                length = node.end_lineno - node.lineno + 1
                self.assertLessEqual(length, MAX_FUNCTION_LINES, f"{node.name} is {length} lines long")

    def test_process_orders_is_thin(self):
        process = self.functions["process_orders"]
        body = process.body
        if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant):
            body = body[1:]
        count = sum(isinstance(n, ast.stmt) for stmt in body for n in ast.walk(stmt))
        self.assertLessEqual(count, MAX_PROCESS_STATEMENTS)

    def test_process_orders_uses_the_pieces(self):
        reachable, frontier = set(), ["process_orders"]
        while frontier:
            name = frontier.pop()
            if name in reachable or name not in self.functions:
                continue
            reachable.add(name)
            frontier.extend(called_names(self.functions[name]))
        for name in REQUIRED[:-1]:
            self.assertIn(name, reachable, f"process_orders never calls {name}")


if __name__ == "__main__":
    unittest.main()
