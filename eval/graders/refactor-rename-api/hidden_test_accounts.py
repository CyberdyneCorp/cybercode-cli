import ast
import unittest
import warnings
from pathlib import Path

import accounts
from accounts import billing, notifications, reports, users

PACKAGE = Path(accounts.__file__).resolve().parent
OLD = "fetch_user_data"


def deprecations(caught):
    return [w for w in caught if issubclass(w.category, DeprecationWarning)]


class HiddenBehaviourTest(unittest.TestCase):
    def test_visible_behaviour_kept(self):
        self.assertEqual(billing.monthly_charge(3), 0)
        self.assertEqual(billing.invoice_header(1), "Invoice for Ada Lovelace (pro): $12")
        self.assertEqual(billing.invoice_header(2), "Invoice for Alan Turing (free): $0")
        self.assertEqual(notifications.greeting(3), "Hello, Grace!")
        self.assertEqual(reports.active_emails([2, 3, 99, 1]), ["alan@example.com", "ada@example.com"])
        self.assertEqual(users.display_name(1), "Ada Lovelace <ada@example.com>")

    def test_get_user(self):
        self.assertIs(accounts.get_user, users.get_user)
        user = users.get_user(2)
        self.assertEqual(user["email"], "alan@example.com")
        user["name"] = "changed"
        self.assertEqual(users.get_user(2)["name"], "Alan Turing")
        with self.assertRaises(accounts.UnknownUser):
            users.get_user(404)

    def test_new_api_does_not_warn(self):
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            users.get_user(1)
            users.display_name(1)
            billing.invoice_header(1)
            notifications.greeting(1)
            reports.active_emails([1, 2, 3, 9])
        self.assertEqual(deprecations(caught), [])

    def test_old_name_still_works_and_warns(self):
        for module in (users, accounts):
            with self.subTest(module=module.__name__):
                with warnings.catch_warnings(record=True) as caught:
                    warnings.simplefilter("always")
                    result = getattr(module, OLD)(1)
                self.assertEqual(result, users.get_user(1))
                found = deprecations(caught)
                self.assertEqual(len(found), 1)
                self.assertIn("get_user", str(found[0].message))
                self.assertEqual(Path(found[0].filename).resolve(), Path(__file__).resolve(),
                                 "the warning must point at the caller")

    def test_old_name_raises_same_error(self):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            with self.assertRaises(accounts.UnknownUser):
                getattr(users, OLD)(404)

    def test_get_user_is_the_implementation(self):
        self.assertIsNot(users.get_user, getattr(users, OLD))


class HiddenCallSiteTest(unittest.TestCase):
    def test_package_no_longer_uses_old_name(self):
        offenders = []
        for path in sorted(PACKAGE.rglob("*.py")):
            tree = ast.parse(path.read_text())
            for node in ast.walk(tree):
                if isinstance(node, ast.alias) and node.name == OLD and path.name != "__init__.py":
                    offenders.append(f"{path.name}:{node.lineno} imports {OLD}")
                elif isinstance(node, ast.Attribute) and node.attr == OLD:
                    offenders.append(f"{path.name}:{node.lineno} uses .{OLD}")
                elif isinstance(node, ast.Name) and node.id == OLD and not (
                        path.name == "users.py" and isinstance(node.ctx, ast.Store)):
                    offenders.append(f"{path.name}:{node.lineno} uses {OLD}")
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
