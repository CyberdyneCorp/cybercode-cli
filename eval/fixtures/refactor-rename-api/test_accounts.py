import unittest

from accounts import UnknownUser
from accounts.billing import invoice_header, monthly_charge
from accounts.notifications import greeting
from accounts.reports import active_emails
from accounts.users import display_name


class AccountsTest(unittest.TestCase):
    def test_billing(self):
        self.assertEqual(monthly_charge(3), 0)
        self.assertEqual(invoice_header(1), "Invoice for Ada Lovelace (pro): $12")

    def test_greeting(self):
        self.assertEqual(greeting(2), "Hello, Alan!")

    def test_reports_skip_unknown_and_inactive(self):
        self.assertEqual(active_emails([3, 99, 1]), ["ada@example.com"])

    def test_unknown_user(self):
        with self.assertRaises(UnknownUser):
            display_name(42)


if __name__ == "__main__":
    unittest.main()
